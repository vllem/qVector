#include "qt/timelineview.h"
#include "qt/avatar.h"
#include "qt/emoji.h"
#include "qt/imageload.h"
#include "qt/theme.h"
#include <QAbstractTextDocumentLayout>
#include <QApplication>
#include <QBuffer>
#include <QClipboard>
#include <QDateTime>
#include <QDesktopServices>
#include <QHBoxLayout>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QRegularExpression>
#include <QScrollBar>
#include <QTextBlock>
#include <QTextTable>
#include <QTimer>
#include <QVBoxLayout>
#include <functional>

namespace vc {

static QString S(const QJsonObject &o, const char *k) { return o.value(QLatin1String(k)).toString(); }
static bool B(const QJsonObject &o, const char *k) { return o.value(QLatin1String(k)).toBool(); }
static QString hex(const QColor &c) { return c.name(); }
static QString esc(const QString &s) { return s.toHtmlEscaped(); }
static QString enc(const QString &s) { return QString::fromLatin1(s.toUtf8().toPercentEncoding()); }

/* A message that is nothing but up to ten emoji (and spaces) is shown large, as other clients do. Greedy longest match against the picker's table, so
   flags, skin tones and joined sequences count as one; a stray variation selector is tolerated. */
static bool emojiOnly(const QString &text)
{
    static QSet<QString> glyphs;
    static int longest = 0;
    if (glyphs.isEmpty()) {
        for (const Emoji &e : allEmoji()) {
            QString g = e.glyph;
            g.remove(QChar(0xFE0F));
            glyphs.insert(g);
            longest = qMax(longest, int(g.size()));
        }
    }
    QString t = text.trimmed();
    t.remove(QChar(0xFE0F)); t.remove(QChar(0xFE0E));
    int count = 0, i = 0;
    while (i < t.size()) {
        if (t[i].isSpace()) { i++; continue; }
        int len = qMin(longest, int(t.size()) - i);
        for (; len > 0 && !glyphs.contains(t.mid(i, len)); len--) {}
        if (len == 0 || ++count > 10) return false;
        i += len;
    }
    return count > 0;
}

/* the part of a web address found in text that belongs to it: trailing punctuation is the sentence's, a closing bracket stays when the link opened one */
static QString trimmedUrl(QString url)
{
    for (;;) {
        if (url.isEmpty()) break;
        const QChar last = url.back();
        const bool trim = last == '.' || last == ',' || last == ';' || last == ':' || last == '!' || last == '?' || last == '\'' ||
                          (last == ')' && url.count('(') < url.count(')'));
        if (!trim) break;
        url.chop(1);
    }
    return url;
}

static QString linkify(const QString &text)
{
    /* a scheme URL, "www.host...", or "host.tld/path" without a scheme (then https is assumed) */
    static const QRegularExpression re(R"((?<![\w@./-])((?:https?://|www\.|[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/)[^\s<>"]+))");
    QString out;
    int last = 0;
    auto it = re.globalMatch(text);
    while (it.hasNext()) {
        auto m = it.next();
        QString url = trimmedUrl(m.captured(1));
        const QString tail = m.captured(1).mid(url.size());
        out += esc(text.mid(last, m.capturedStart() - last));
        const QString href = url.startsWith("http://") || url.startsWith("https://") ? url : "https://" + url;
        out += "<a href=\"" + esc(href) + "\">" + esc(url) + "</a>" + esc(tail);
        last = m.capturedEnd();
    }
    out += esc(text.mid(last));
    out.replace('\n', "<br>");
    return out;
}

/* a thin frame painted into a picture (device pixels). Its colour is the
 * inverse of the picture's average colour (sampled on a coarse grid), pushed to
 * black or white when that would blend into the picture or the timeline background */
static void frameImage(QImage &img, const QColor &bg, qreal dpr)
{
    if (img.isNull()) return;
    qint64 r = 0, g = 0, b = 0, n = 0;
    const int sx = qMax(1, img.width() / 16), sy = qMax(1, img.height() / 16);
    for (int y = 0; y < img.height(); y += sy)
        for (int x = 0; x < img.width(); x += sx) {
            const QColor px = img.pixelColor(x, y);
            if (px.alpha() < 128) continue;
            r += px.red(); g += px.green(); b += px.blue(); n++;
        }
    const QColor avg = n ? QColor(int(r / n), int(g / n), int(b / n)) : bg;
    QColor c(255 - avg.red(), 255 - avg.green(), 255 - avg.blue());
    auto lum = [](const QColor &k) { return qGray(k.rgb()); };
    auto gap = [&](const QColor &k) { return qMin(qAbs(lum(k) - lum(avg)), qAbs(lum(k) - lum(bg))); };
    if (gap(c) < 64) {
        const QColor dark(0, 0, 0), light(255, 255, 255);
        c = gap(dark) > gap(light) ? dark : light;
    }
    QPainter fp(&img);
    fp.setPen(QPen(c, qMax(1.0, dpr)));
    fp.setBrush(Qt::NoBrush);
    const qreal h = qMax(1.0, dpr) / 2;
    fp.drawRect(QRectF(h, h, img.width() - 2 * h, img.height() - 2 * h));
}

/* `img` scaled into `box`; a null image stays null (QImage::scaled complains about those, and a picture whose file cannot be read is not an error) */
static QImage scaledInto(const QImage &img, const QSize &box, Qt::AspectRatioMode mode)
{
    return img.isNull() ? QImage() : img.scaled(box, mode, Qt::SmoothTransformation);
}

static QHash<QUrl, QImage> *g_resStore = nullptr; /* the pictures of the render in progress: a new document gets them all */

/* adds a picture to the document and folds its pixels into a running hash, so a redraw can tell whether anything visible changed */
static void addRes(QTextDocument *doc, quint64 &hash, const QUrl &url, const QImage &img)
{
    doc->addResource(QTextDocument::ImageResource, url, img);
    if (g_resStore) g_resStore->insert(url, img);
    hash += (quint64(qHash(url.toString())) * 1099511628211ULL) ^ (quint64(qHashBits(img.constBits(), size_t(img.sizeInBytes()))) * 31);
}

TimelineView::TimelineView(bool thread, QWidget *parent) : QWidget(parent), thread_(thread)
{
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(0, 0, 0, 0);
    v->setSpacing(0);
    view_ = new QTextBrowser;
    view_->setOpenLinks(false);
    view_->setFrameShape(QFrame::NoFrame);
    view_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff); /* messages wrap; a scrollbar for the whole room because of one wide message is never what is wanted */
    view_->document()->setDocumentMargin(6);
    applyStyle();
    v->addWidget(view_, 1);
    connect(view_, &QTextBrowser::anchorClicked, this, &TimelineView::onAnchor);
    view_->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(view_, &QWidget::customContextMenuRequested, this, &TimelineView::contextMenu);
    connect(view_->verticalScrollBar(), &QScrollBar::valueChanged, this, [this] { onScroll(); placePlayer(); });
    connect(view_->verticalScrollBar(), &QScrollBar::rangeChanged, this, [this](int, int) {
        /* content grew (image arrived, window resized): keep following the bottom */
        if (stick_ && !loadingMore_) {
            programmatic_ = true;
            view_->verticalScrollBar()->setValue(view_->verticalScrollBar()->maximum());
            programmatic_ = false;
            QTimer::singleShot(0, this, [this] { programmatic_ = true; view_->verticalScrollBar()->setValue(view_->verticalScrollBar()->maximum()); programmatic_ = false; });
        }
    });
    view_->viewport()->installEventFilter(this);
    view_->verticalScrollBar()->installEventFilter(this);
    view_->installEventFilter(this);
}

static const int kMaxShownChars = 20000;
/* Laying out an emoji costs about a millisecond (Qt asks the colour font for the bitmap of every glyph it places; a letter takes a few microseconds): 500 flags took 0.4 s on every
   redraw of the room, the 5 000 that fit in kMaxShownChars 5 s, and a room with such a message was slow to open and to scroll. A text is laid out up to its kMaxShownEmoji-th emoji. */
static const int kMaxShownEmoji = 400;

/* Where the text has its kMaxShownEmoji + 1st emoji (the place to cut it), or -1. A flag is two code points (regional indicators): the cut never falls between them. */
static int emojiCut(const QString &t)
{
    int count = 0, indicators = 0; /* the regional indicators just before this place */
    for (int i = 0; i < t.size(); i++) {
        uint cp = t[i].unicode();
        int len = 1;
        if (QChar::isHighSurrogate(cp) && i + 1 < t.size() && t[i + 1].isLowSurrogate()) { cp = QChar::surrogateToUcs4(QChar(cp), t[i + 1]); len = 2; }
        const bool indicator = cp >= 0x1F1E6 && cp <= 0x1F1FF;
        if (((cp >= 0x1F000 && cp <= 0x1FAFF) || (cp >= 0x2600 && cp <= 0x27BF)) && ++count > kMaxShownEmoji) return indicator && (indicators & 1) ? i - 2 : i;
        indicators = indicator ? indicators + 1 : 0;
        i += len - 1;
    }
    return -1;
}
static const int kMinDrawnBytes = 24 << 20, kMaxDrawnBytes = 512 << 20; /* the picture cache of a view (see drawn_): never below / above this */

TimelineView::~TimelineView() { stopInline(); }

void TimelineView::reset()
{
    rows_ = QJsonArray();
    rowIndex_.clear();
    lastChunks_.clear(); rowEvents_.clear(); lastState_.clear();
    windowStart_.clear(); windowEnd_.clear(); hiddenLocal_ = shownEnd_ = 0; shownLimit_ = 100; stick_ = true; loadingMore_ = false; highlight_.clear();
    stopInline();
    canLoadMore_ = false; loadingHistory_ = false;
    programmatic_ = true;
    view_->setHtml(QString());
    programmatic_ = false;
}

void TimelineView::stickToBottom()
{
    stick_ = true;
    if (!windowEnd_.isEmpty()) { windowEnd_.clear(); windowStart_.clear(); refresh(); } /* a document that stops short of the newest message follows it again */
}

void TimelineView::setRows(const QJsonArray &rows)
{
    /* id -> position, so looking up a reply target or a jump does not walk 50 000 rows. A message arriving at the end only adds its own entry. */
    const int old = rows_.size();
    const bool grew = old > 0 && rows.size() >= old && rows[old - 1].toObject().value("id") == rows_[old - 1].toObject().value("id")
        && rows[0].toObject().value("id") == rows_[0].toObject().value("id");
    /* Older messages arrived (rows were added in front): show them now. Left to the window rule they would sit folded away above until the next
       scroll, and the reader would see nothing happen. */
    const int added = old > 0 && !grew && rows.size() > old && rows[rows.size() - 1].toObject().value("id") == rows_[old - 1].toObject().value("id")
        ? rows.size() - old : 0;
    if (added > 0 && loadingMore_ && !thread_) windowStart_ = rows[0].toObject().value("id").toString(); /* the document starts with them (and drops its newest rows once it is over kWindowMax) */
    rows_ = rows;
    if (!grew) rowIndex_.clear();
    for (int i = grew ? old : 0; i < rows.size(); i++) rowIndex_.insert(rows[i].toObject().value("id").toString(), i); /* first of equal ids wins */
    refresh();
    if (!seeking_.isEmpty() && !row(seeking_).isEmpty()) { const QString id = seeking_; seeking_.clear(); seekPending_ = false; revealMessage(id); }
}

/* Going to a message that is older than what is loaded: load older history a page at a time until it appears (or the room has no more). */
void TimelineView::continueSeek()
{
    if (seeking_.isEmpty()) return;
    seekPending_ = false;
    if (!row(seeking_).isEmpty()) { const QString id = seeking_; seeking_.clear(); revealMessage(id); return; }
    if (canLoadMore_ && seekTries_++ < 40) { seekPending_ = true; loadingMore_ = true; emit olderRequested(); }
    else seeking_.clear();
}

QJsonObject TimelineView::row(const QString &eventId) const
{
    const auto it = rowIndex_.constFind(eventId);
    return it == rowIndex_.constEnd() ? QJsonObject() : rows_[it.value()].toObject();
}

/* Only the reader's own scrolling counts (wheel, scrollbar, keys). The scrollbar also moves when the document is laid out or redrawn; treating that as
   "the reader went to the top" is what made the view load more, redraw and jump while a chat was being opened. */
bool TimelineView::eventFilter(QObject *obj, QEvent *ev)
{
    switch (ev->type()) {
    case QEvent::Wheel: case QEvent::MouseButtonPress: case QEvent::KeyPress: case QEvent::TouchBegin: case QEvent::TouchUpdate:
        userInput_.start();
        break;
    case QEvent::MouseMove:
        if (static_cast<QMouseEvent *>(ev)->buttons() != Qt::NoButton) userInput_.start();
        break;
    default: break;
    }
    return QWidget::eventFilter(obj, ev);
}

void TimelineView::onScroll()
{
    if (programmatic_) return;
    if (!userInput_.isValid() || userInput_.elapsed() > 700) return; /* not the reader: a layout change */
    QScrollBar *sb = view_->verticalScrollBar();
    const bool following = windowEnd_.isEmpty(); /* the document reaches the room's newest message (the rows may have grown since it was drawn) */
    stick_ = sb->value() >= sb->maximum() - 4 && following; /* the bottom of a document that stops short of the newest message is not the end of the room */
    fitFetches_ = 0;
    if (sb->value() <= 30) {
        if (hiddenLocal_ > 0) showEarlier();
        else if (canLoadMore_ && !loadingHistory_ && !thread_) { loadingMore_ = true; emit olderRequested(); }
    } else if (sb->value() >= sb->maximum() - 30 && !following) showLater();
}

/* The document is a window on the room's messages (laying one out takes about 1 ms, and it is 6 KB in memory): these two move it, a hundred messages at a time,
   and it never holds more than kWindowMax. The reader stays where they were (render() puts the message at the top of the view back). */
void TimelineView::showEarlier()
{
    if (hiddenLocal_ <= 0) return;
    const int first = qMax(0, hiddenLocal_ - 100);
    windowStart_ = S(rows_[first].toObject(), "id");
    if (shownEnd_ - first > kWindowMax) windowEnd_ = S(rows_[first + kWindowMax].toObject(), "id");
    loadingMore_ = true;
    render();
}

void TimelineView::showLater()
{
    const int n = rows_.size();
    if (windowEnd_.isEmpty() || shownEnd_ >= n) return;
    const int last = qMin(n, shownEnd_ + 100);
    windowEnd_ = last >= n ? QString() : S(rows_[last].toObject(), "id");
    if (last - hiddenLocal_ > kWindowMax) windowStart_ = S(rows_[last - kWindowMax].toObject(), "id");
    loadingMore_ = true;
    render();
}

void TimelineView::onAnchor(const QUrl &u)
{
    const QString s = u.toString();
    auto part = [&](int n) { return QUrl::fromPercentEncoding(s.split('/').value(n).toUtf8()); };
    if (s == "vc:more") {
        if (hiddenLocal_ > 0) showEarlier();
        else { loadingMore_ = true; emit olderRequested(); }
    }
    else if (s == "vc:newer") { stick_ = true; windowEnd_.clear(); windowStart_.clear(); loadingMore_ = false; render(); } /* back to the newest messages */
    else if (s.startsWith("vc:img:") || s.startsWith("vc:open:")) emit openRequested(QUrl::fromPercentEncoding(s.mid(s.indexOf(':', 3) + 1).toUtf8())); /* a gallery item is "<event>#<n>" */
    else if (s.startsWith("vc:save:")) emit saveRequested(QUrl::fromPercentEncoding(s.mid(8).toUtf8()));
    else if (s.startsWith("vc:spoiler/")) { const QString k = QUrl::fromPercentEncoding(s.mid(11).toUtf8()); if (!revealed_.remove(k)) revealed_.insert(k); render(); }
    else if (s.startsWith("vc:text:")) emit textRequested(s.mid(8));
    else if (s.startsWith("vc:thread:")) emit threadRequested(s.mid(10));
    else if (s.startsWith("vc:goto/")) { /* vc:goto/<answered event>[/<the reply, when the original is not loaded>] */
        const QString target = part(1), reply = part(2);
        if (!reply.isEmpty()) emit loadReplyRequested(reply);
        revealMessage(target);
    }
    else if (s.startsWith("vc:vid:")) {
        const QString id = s.mid(7);
        if (id == loadingId_) return; /* already downloading: the card shows a spinner */
        if (row(id).value("kind").toString() == "video") setLoading(id);
        emit playRequested(id);
    }
    else if (s.startsWith("https://matrix.to/#/") || s.startsWith("http://matrix.to/#/")) emit matrixLink(s);
    else if (s.startsWith("vc:poll/")) emit pollVote(part(1).mid(0), part(2)); /* vc:poll/<event>/<answer> */
    else if (s.startsWith("vc:pollend/")) emit pollEnd(QUrl::fromPercentEncoding(s.mid(11).toUtf8()));
    else if (s.startsWith("vc:react/")) emit reactRequested(part(1), part(2));
    else QDesktopServices::openUrl(u);
}

void TimelineView::refresh()
{
    if (!isVisible()) { stale_ = true; return; } /* rendering a room nobody looks at is wasted work */
    /* Laying the document out takes tens to hundreds of milliseconds: a burst of changes is drawn once. */
    const qint64 since = lastRender_.isValid() ? lastRender_.elapsed() : 1000;
    if (since < 150 && !loadingMore_) { /* the reader is waiting at the top for older messages: no merging */
        if (!renderQueued_) {
            renderQueued_ = true;
            QTimer::singleShot(int(150 - since), this, [this] { renderQueued_ = false; if (isVisible()) render(); });
        }
        return;
    }
    render();
}

void TimelineView::showEvent(QShowEvent *e)
{
    QWidget::showEvent(e);
    if (stale_) {
        render();
        if (!pendingReveal_.isEmpty()) { const QString id = pendingReveal_; pendingReveal_.clear(); revealMessage(id); }
    }
}

/* Where an event's row starts in the document (scrollbar units), found by letting the browser scroll to its anchor (0 when it is not there). */
int TimelineView::anchorY(const QString &eventId)
{
    QScrollBar *sb = view_->verticalScrollBar();
    const int old = sb->value();
    QSignalBlocker block(sb);
    sb->setValue(0);
    view_->scrollToAnchor("ev" + QString::number(qHash(eventId)));
    const int y = sb->value();
    sb->setValue(old);
    return y;
}

void TimelineView::render()
{
    stale_ = false;
    lastRender_.start();
    /* What the reader is looking at: the message at the top of the view and how far into it the view is. This is read now, while the rows and the
       maps of the document still describe what is shown; after the redraw that message is put back at the same place, so the text does not jump. */
    QString topEvent;
    int delta = 0;
    if (!stick_) {
        for (int y : {2, 14, 30, 60, 100}) { topEvent = eventAt(QPoint(24, y)); if (!topEvent.isEmpty()) break; }
        if (!topEvent.isEmpty()) delta = view_->verticalScrollBar()->value() - anchorY(topEvent);
    }
    const QPalette pal = palette();
    const QString muted = hex(pal.color(QPalette::PlaceholderText)), link = hex(pal.color(QPalette::Link));
    QTextDocument *doc = view_->document();
    const qreal dpr = devicePixelRatioF();
    /* A picture is drawn small: a photo at most 480 x 320, a sticker 160, a link's picture 96, a custom emoji as high as text. The file is decoded
       straight to that size, the finished picture (scaled, framed) is kept for the next redraw (every new message redraws the pictures in view)
       and nothing else of it: it used to be a 1200 pixel copy of every file ever shown, and a redraw scaled them all again. The cache only has to
       hold what is in view now plus some pictures of rooms visited lately: its limit follows what this redraw used. */
    const QColor base = pal.color(QPalette::Base);
    const QString look = QString::number(dpr) + QChar(1) + base.name();
    qint64 drawnBytes = 0;
    drawn_.setMaxCost(kMaxDrawnBytes); /* never evict what this redraw has just made */
    /* a redraw that decoded files has made (and freed) buffers of megabytes: the heap gives them back when this redraw is over (see giveBackFreedMemory) */
    struct TrimAtEnd { bool decoded = false; ~TrimAtEnd() { if (decoded) giveBackFreedMemory(); } } trimAtEnd;
    auto drawn = [&](const QString &what, const QString &path, const std::function<QImage(const QString &)> &make) -> QImage {
        if (path.isEmpty()) return QImage();
        const QString key = what + QChar(1) + path + QChar(1) + look;
        if (const QImage *hit = drawn_.object(key)) { drawnBytes += hit->sizeInBytes(); return *hit; }
        trimAtEnd.decoded = true;
        const QImage img = make(path);
        const qsizetype bytes = img.isNull() ? 1 : img.sizeInBytes(); /* a file that cannot be read is remembered too, at no cost */
        drawn_.insert(key, new QImage(img), int(qMin<qsizetype>(bytes, kMaxDrawnBytes)));
        drawnBytes += bytes;
        return img;
    };

    rowEvents_.clear();
    resHash_ = 0;
    resources_.clear();
    g_resStore = &resources_;
    applyStyle();
    for (int k = 0; k < 3; k++) addRes(doc, resHash_, QUrl(QString("shield:%1").arg(k)), shieldPixmap(k, 14, dpr).toImage());
    QString html; /* the rows; the document is a sequence of small tables, one per message, cut at the \x02 marks */
    QVector<int> chunkStart;
    const int n = rows_.size();
    /* Laying out a long room is what costs time (about 1 ms per message) and a message in the document is about 6 KB, so the document is a window on the
       room's messages, [firstShown, lastShown). `windowStart_` / `windowEnd_` are the message it starts with and the one it stops before (ids: older messages
       arriving in front change nothing); no end follows the room's newest message. Following the end it is the newest `shownLimit_` messages, cut back to that
       once it has grown to twice as many. Scrolling to the top adds messages above and, past kWindowMax, drops the newest; scrolling to the bottom of a window that
       stops short of the end adds messages below and drops the oldest (showEarlier, showLater). The first message shown stays the same while new ones are added
       at the end, so the tables above never change. */
    int firstShown = 0, lastShown = n;
    auto indexOf = [&](const QString &id) { const auto it = rowIndex_.constFind(id); return it == rowIndex_.constEnd() ? -1 : it.value(); };
    if (!windowEnd_.isEmpty()) { const int e = indexOf(windowEnd_); if (e > 0 && e < n) lastShown = e; else windowEnd_.clear(); }
    if (lastShown < n || n > int(shownLimit_)) {
        int idx = windowStart_.isEmpty() ? -1 : indexOf(windowStart_);
        if (idx >= lastShown) idx = -1;
        if (lastShown == n) { /* the window reaches the newest message */
            if (idx < 0 || (stick_ && n - idx > int(shownLimit_) * 2)) idx = n - int(shownLimit_);
            else if (n - idx > kWindowMax) { lastShown = idx + kWindowMax; windowEnd_ = S(rows_[lastShown].toObject(), "id"); } /* the reader keeps their place, the newest messages wait below */
        } else { /* it stops short of the newest message */
            if (idx < 0) idx = qMax(0, lastShown - int(shownLimit_));
            if (lastShown - idx > kWindowMax) { lastShown = idx + kWindowMax; windowEnd_ = S(rows_[lastShown].toObject(), "id"); }
        }
        firstShown = idx;
        windowStart_ = S(rows_[idx].toObject(), "id");
    } else windowStart_.clear();
    hiddenLocal_ = firstShown;
    shownEnd_ = lastShown;
    if (!thread_ && (firstShown || canLoadMore_)) { /* older messages exist: a link at the top (scrolling there loads them too) */
        rowEvents_ << QString();
        html += "<tr><td colspan=3 align=center style=\"padding:6px\">" +
                (firstShown ? QString("<a href=\"vc:more\">Show earlier messages</a>")
                            : loadingHistory_ ? QString("<font color=\"" + muted + "\">Loading older messages...</font>")
                                              : QString("<a href=\"vc:more\">Load older messages</a>")) + "</td></tr>";
    } else if (!thread_ && n > 0) {
        rowEvents_ << QString();
        html += "<tr><td colspan=3 align=center style=\"padding:6px\"><font color=\"" + muted + "\">This is the start of the conversation</font></td></tr>";
    }
    QSet<QString> haveAvatars;
    QString lastSender;
    bool lastMedia = false; /* the previous message was a picture or video */
    qint64 lastTs = 0;
    for (int i = firstShown; i < lastShown; i++) {
        const QJsonObject r = rows_[i].toObject();
        const QString kind = S(r, "kind"), eid = S(r, "id"), sender = S(r, "sender_id"), name = S(r, "sender");
        QString text = S(r, "body");
        /* A multi-megabyte message makes the text layout run for minutes and freezes the window, so only the start is drawn (and as many emoji as kMaxShownEmoji). */
        const int emojiAt = emojiCut(text), shownChars = emojiAt >= 0 ? qMin(emojiAt, kMaxShownChars) : kMaxShownChars;
        if (text.size() > shownChars) text = text.left(shownChars) + QString::fromUtf8("\u2026 (message cut, %1 characters in all)").arg(text.size());
        const qint64 ts = qint64(r.value("ts").toDouble());
        const bool pending = B(r, "pending");
        const bool grouped = sender == lastSender && ts - lastTs < 5 * 60 * 1000 && ts >= lastTs;
        lastSender = sender; lastTs = ts;
        const bool media = kind == "image" || kind == "sticker" || kind == "gallery" || kind == "video";
        const bool gapAbove = grouped && (media || lastMedia); /* pictures sent one after another need a gap between them */
        lastMedia = media;
        const QDateTime dt = QDateTime::fromMSecsSinceEpoch(ts);
        QString when = formatWhen(dt);
        if (bookmarks_.contains(eid)) when = QStringLiteral("★︎ ") + when; /* saved by the user */
        if (pinned_.contains(eid) || B(r, "pinned")) when = QStringLiteral("pinned · ") + when;

        QString content;
        const QJsonObject poll = r.value("poll").toObject();
        if (kind == "poll" && !poll.isEmpty()) { /* a poll: its answers are links, the tally is drawn as bars */
            const bool ended = B(poll, "ended");
            const QJsonArray answers = poll.value("answers").toArray();
            const int maxSel = poll.value("max_selections").toInt(1), total = poll.value("total_votes").toInt();
            int most = 1;
            for (const QJsonValue &a : answers) most = qMax(most, a.toObject().value("votes").toInt());
            content = "<b>" + esc(S(poll, "question")) + "</b><br>";
            for (const QJsonValue &av : answers) {
                const QJsonObject a = av.toObject();
                QString line = (B(a, "mine") ? QStringLiteral("●") : QStringLiteral("○")) + " " + esc(S(a, "text"));
                if (!ended && !eid.isEmpty() && !pending) line = "<a href=\"vc:poll/" + enc(eid) + "/" + enc(S(a, "id")) + "\" style=\"text-decoration:none\">" + line + "</a>";
                line += " &nbsp;<span style=\"color:" + link + "\">" + QString(a.value("votes").toInt() * 20 / most, QChar(0x2588)) + "</span> " + QString::number(a.value("votes").toInt());
                content += line + "<br>";
            }
            QString foot = ended ? "Poll ended" : maxSel > 1 ? QString("Choose up to %1").arg(maxSel) : QString("Choose one");
            foot += QString(" · %1 %2").arg(total).arg(total == 1 ? "vote" : "votes");
            content += "<span style=\"color:" + muted + "\">" + esc(foot) + "</span>";
            if (!ended && !eid.isEmpty() && B(r, "own")) content += " <a href=\"vc:pollend/" + enc(eid) + "\">End poll</a>";
        } else if (kind == "image") {
            const QSize box = QSize(480, 320) * dpr;
            const QImage scaled = drawn("img", S(r, "image_path"), [&](const QString &p) {
                QImage s = scaledInto(loadFitted(p, box), box, Qt::KeepAspectRatio);
                frameImage(s, base, dpr);
                s.setDevicePixelRatio(dpr);
                return s;
            });
            if (!scaled.isNull()) {
                const QString key = "img:" + eid;
                addRes(doc, resHash_, QUrl(key), scaled);
                content = "<a href=\"vc:img:" + esc(eid) + "\"><img src=\"" + key + "\" width=" + QString::number(int(scaled.width() / dpr)) + "></a>";
            } else content = "<span style=\"color:" + muted + "\">Loading image...</span>";
            const QString fn = S(r, "file_name");
            if (!text.isEmpty() && text != "image" && text != fn) content += "<br>" + esc(text);
        } else if (kind == "sticker") { /* a picture without a frame, smaller than a photo */
            const QSize box = QSize(160, 160) * dpr;
            const QImage scaled = drawn("stk", S(r, "image_path"), [&](const QString &p) {
                QImage s = scaledInto(loadFitted(p, box), box, Qt::KeepAspectRatio);
                s.setDevicePixelRatio(dpr);
                return s;
            });
            if (!scaled.isNull()) {
                const QString key = "stk:" + eid;
                addRes(doc, resHash_, QUrl(key), scaled);
                content = "<img src=\"" + key + "\" width=" + QString::number(int(scaled.width() / dpr)) + " title=\"" + esc(text) + "\">";
            } else content = "<span style=\"color:" + muted + "\">Loading sticker...</span>";
        } else if (kind == "gallery") { /* several pictures (or files) in one message: a grid of thumbnails, cropped to squares like Discord's */
            const QJsonArray items = r.value("gallery").toArray();
            QVector<QJsonObject> pics, others;
            for (const QJsonValue &v : items) (S(v.toObject(), "kind") == "image" ? pics : others).append(v.toObject());
            const int n = pics.size(), cols = n <= 1 ? 1 : (n == 2 || n == 4) ? 2 : 3, gap = 4, total = 480;
            const int side = n == 1 ? total : (total - gap * (cols - 1)) / cols;
            if (n) {
                content = "<table cellspacing=" + QString::number(gap) + " cellpadding=0>";
                for (int i = 0; i < n; i += cols) {
                    content += "<tr>";
                    for (int c = 0; c < cols; c++) {
                        if (i + c >= n) { content += "<td></td>"; continue; }
                        const QJsonObject it = pics[i + c];
                        const QString href = "vc:img:" + esc(eid) + "%23" + QString::number(it.value("index").toInt());
                        const QImage t = drawn(n == 1 ? QStringLiteral("gal1") : "gal" + QString::number(side), S(it, "image_path"), [&](const QString &p) {
                            QImage s;
                            if (n == 1) { const QSize box = QSize(total, 320) * dpr; s = scaledInto(loadFitted(p, box), box, Qt::KeepAspectRatio); }
                            else {
                                const QSize box = QSize(side, side) * dpr;
                                const QImage big = scaledInto(loadFitted(p, box, Qt::KeepAspectRatioByExpanding), box, Qt::KeepAspectRatioByExpanding);
                                if (!big.isNull()) s = big.copy((big.width() - side * dpr) / 2, (big.height() - side * dpr) / 2, side * dpr, side * dpr);
                            }
                            frameImage(s, base, dpr);
                            s.setDevicePixelRatio(dpr);
                            return s;
                        });
                        if (t.isNull()) { content += "<td width=" + QString::number(side) + " height=" + QString::number(side * 2 / 3) + " bgcolor=\"" + hex(pal.color(QPalette::AlternateBase)) + "\" align=center><span style=\"color:" + muted + "\">Loading...</span></td>"; continue; }
                        const QString key = "gal:" + eid + "#" + QString::number(it.value("index").toInt());
                        addRes(doc, resHash_, QUrl(key), t);
                        content += "<td><a href=\"" + href + "\"><img src=\"" + key + "\" width=" + QString::number(int(t.width() / dpr)) + "></a></td>";
                    }
                    content += "</tr>";
                }
                content += "</table>";
            }
            for (const QJsonObject &it : others)
                content += "<a href=\"vc:save:" + esc(eid) + "%23" + QString::number(it.value("index").toInt()) + "\">&#128206; " + esc(S(it, "name")) + "</a> <span style=\"color:" + muted + "\">" + esc(QLocale().formattedDataSize(it.value("size").toVariant().toLongLong())) + "</span><br>";
            if (!text.isEmpty()) content += (content.isEmpty() ? "" : "<br>") + esc(text);
        } else if (kind == "video") {
            QImage card(QSize(320, 180) * dpr, QImage::Format_RGB32);
            card.fill(QColor(0x20, 0x20, 0x20));
            const bool playingThis = eid == playingId_;
            if (playingThis) { /* the player widget is laid over the card: leave room for its control row */
                QImage tall(card.width(), card.height() + int(VideoPlayer::kControlsHeight * dpr), QImage::Format_RGB32);
                tall.fill(Qt::black);
                QPainter tp(&tall);
                tp.drawImage(0, 0, card);
                tp.end();
                card = tall;
                playingSize_ = QSize(int(card.width() / dpr), int(card.height() / dpr));
            }
            QPainter p(&card);
            p.setRenderHint(QPainter::Antialiasing);
            const QPointF c(card.width() / 2.0, (card.height() - (playingThis ? VideoPlayer::kControlsHeight * dpr : 0)) / 2.0);
            const qreal rad = 26 * dpr;
            if (!playingThis && eid == loadingId_) {
                p.setBrush(QColor(0, 0, 0, 150)); p.setPen(Qt::NoPen);
                p.drawEllipse(c, rad, rad);
                p.setBrush(Qt::NoBrush);
                p.setPen(QPen(Qt::white, 3 * dpr, Qt::SolidLine, Qt::RoundCap));
                const QRectF arc(c.x() - rad * 0.6, c.y() - rad * 0.6, rad * 1.2, rad * 1.2);
                p.drawArc(arc, -spinStep_ * 30 * 16, -270 * 16);
                p.setPen(Qt::white);
                QFont f = p.font(); f.setPixelSize(int(12 * dpr)); p.setFont(f);
                p.drawText(QRectF(0, c.y() + rad + 6 * dpr, card.width(), 18 * dpr), Qt::AlignHCenter, "Loading video…");
            } else if (!playingThis) {
                p.setBrush(QColor(0, 0, 0, 150)); p.setPen(Qt::NoPen);
                p.drawEllipse(c, rad, rad);
                p.setBrush(Qt::white);
                QPolygonF tri; tri << QPointF(c.x() - rad * 0.3, c.y() - rad * 0.45) << QPointF(c.x() - rad * 0.3, c.y() + rad * 0.45) << QPointF(c.x() + rad * 0.5, c.y());
                p.drawPolygon(tri);
            }
            p.end();
            if (!playingThis) frameImage(card, pal.color(QPalette::Base), dpr);
            card.setDevicePixelRatio(dpr);
            const QString key = "vid:" + eid;
            addRes(doc, resHash_, QUrl(key), card);
            content = "<a href=\"vc:vid:" + esc(eid) + "\"><img src=\"" + key + "\" width=" + QString::number(int(card.width() / dpr)) + "></a><br>" + esc(text);
        } else if (kind == "file" || kind == "audio") {
            const QString t = (kind == "file" ? "File: " : "Audio: ") + (S(r, "file_name").isEmpty() ? text : S(r, "file_name"));
            content = pending || eid.isEmpty() ? esc(t) : QString("<a href=\"") + (kind == "audio" ? "vc:vid:" : "vc:save:") + esc(eid) + "\">" + esc(t) + "</a>" + (kind == "audio" ? " &nbsp;<a href=\"vc:vid:" + esc(eid) + "\">Play</a>" : QString());
            const qint64 sz = qint64(r.value("size").toDouble());
            if (kind == "file" && sz > 0) content += " <span style=\"color:" + muted + "\">(" + (sz < 1024 ? QString::number(sz) + " B" : sz < 1024 * 1024 ? QString::number((sz + 1023) / 1024) + " KB" : QString::number(double(sz) / (1024 * 1024), 'f', 1) + " MB") + ")</span>";
            if (kind == "file" && B(r, "text_file") && !pending && !eid.isEmpty()) /* a text file can be read here: a button under it */
                content += "<br><a href=\"vc:text:" + esc(eid) + "\" style=\"text-decoration:none\"><span style=\"background-color:" + hex(pal.color(QPalette::Button)) + ";color:" + hex(pal.color(QPalette::ButtonText)) + "\">&nbsp;&nbsp;Open file&nbsp;&nbsp;</span></a>";
        } else if (kind == "location") {
            content = esc(text);
        } else {
            QString t = text, style;
            if (kind == "undecryptable") { t = "Unable to decrypt this message yet (waiting for the key)"; style = "color:" + muted; }
            else if (kind == "emote") t = "* " + name + " " + text;
            if (kind == "notice" || kind == "emote" || pending) style = "color:" + muted;
            QString shown = linkify(t);
            const QString fb = S(r, "html").size() > 4 * kMaxShownChars || emojiCut(S(r, "html")) >= 0 ? QString() : S(r, "html"); /* too much of either: the plain text, cut, instead */
            if (kind == "text" && fb.isEmpty() && emojiOnly(text)) shown = "<span style=\"font-size:30pt\">" + esc(text) + "</span>";
            if (!fb.isEmpty() && kind != "undecryptable") { /* formatted text: sanitized HTML */
                shown = fb;
                for (const QJsonValue &ev : r.value("emoji").toArray()) { /* custom emoji: the downloaded picture at text height, else its shortcode */
                    const QJsonObject e = ev.toObject();
                    const QString mxc = S(e, "mxc");
                    const int line = int(22 * dpr);
                    const QImage scaled = drawn("emo", S(e, "path"), [&](const QString &p) {
                        const QImage got = loadFitted(p, QSize(1 << 20, line));
                        QImage s = got.isNull() ? got : got.scaledToHeight(line, Qt::SmoothTransformation);
                        s.setDevicePixelRatio(dpr);
                        return s;
                    });
                    QString repl;
                    if (!scaled.isNull()) {
                        const QString key = "emo:" + mxc;
                        addRes(doc, resHash_, QUrl(key), scaled);
                        repl = "<img src=\"" + key + "\" height=22 style=\"vertical-align:middle\">";
                    }
                    const QRegularExpression re("<img[^>]*src=\"" + QRegularExpression::escape(mxc) + "\"[^>]*>");
                    int at = 0;
                    for (QRegularExpressionMatch m = re.match(shown, at); m.hasMatch(); m = re.match(shown, at)) {
                        QString alt = repl;
                        if (alt.isEmpty()) { const QRegularExpressionMatch am = QRegularExpression("alt=\"([^\"]*)\"").match(m.captured(0)); alt = am.hasMatch() ? am.captured(1) : QString(); }
                        shown.replace(m.capturedStart(), m.capturedLength(), alt);
                        at = m.capturedStart() + alt.size();
                    }
                }
                /* the pictures that are left (not ours, "emo:") are other people's: Qt would draw an empty box as wide as the sender said (width="100000" made the whole view
                   100 000 pixels wide), and a picture that is not one of ours has no file here to show anyway: its description stands in for it */
                static const QRegularExpression strayImage("<img\\b(?![^>]*\\bsrc=\"emo:)[^>]*>");
                for (QRegularExpressionMatch m = strayImage.match(shown); m.hasMatch(); m = strayImage.match(shown)) {
                    const QRegularExpressionMatch am = QRegularExpression("\\balt=\"([^\"]*)\"").match(m.captured(0));
                    shown.replace(m.capturedStart(), m.capturedLength(), am.hasMatch() ? am.captured(1) : QString());
                }
                shown.replace("<del>", "<s>").replace("</del>", "</s>"); /* Qt knows <s> */
                /* spoilers (data-mx-spoiler) stay hidden until clicked (spec: Spoiler messages); the span is closed by counting nested spans, so
                   markup inside cannot end it early and show the text */
                static const QRegularExpression spoilerOpen("<span\\s+data-mx-spoiler(?:=(?:\"([^\"]*)\"|'([^']*)'))?\\s*>");
                for (int n = 0; n < 200; n++) {
                    const QRegularExpressionMatch m = spoilerOpen.match(shown);
                    if (!m.hasMatch()) break;
                    int depth = 1, at = m.capturedEnd();
                    while (depth > 0) {
                        const int open = shown.indexOf("<span", at), close = shown.indexOf("</span>", at);
                        if (close < 0) { at = shown.size(); depth = 0; break; }
                        if (open >= 0 && open < close) { depth++; at = open + 5; } else { depth--; at = close + 7; }
                    }
                    const int innerEnd = depth == 0 && at >= 7 && shown.mid(at - 7, 7) == "</span>" ? at - 7 : shown.size();
                    const QString inner = shown.mid(m.capturedEnd(), innerEnd - m.capturedEnd()), reason = m.captured(1) + m.captured(2);
                    const QString key = eid + "/" + QString::number(n);
                    const QString repl = revealed_.contains(key)
                        ? "<a href=\"vc:spoiler/" + esc(key) + "\" style=\"text-decoration:none;color:" + hex(pal.color(QPalette::Text)) + ";background-color:" + hex(pal.color(QPalette::Button)) + "\">" + inner + "</a>"
                        : "<a href=\"vc:spoiler/" + esc(key) + "\" style=\"text-decoration:none;color:" + hex(pal.color(QPalette::Window)) + ";background-color:" + hex(pal.color(QPalette::WindowText)) + "\">&nbsp;" + (reason.isEmpty() ? QStringLiteral("spoiler") : esc(reason)) + "&nbsp;</a>";
                    shown.replace(m.capturedStart(), at - m.capturedStart(), repl);
                }
                /* colours (data-mx-color / data-mx-bg-color) are not CSS for Qt */
                static const QRegularExpression colour("<(font|span|a)([^>]*?)\\sdata-mx-(bg-)?color=\"(#[0-9a-fA-F]{6})\"");
                for (QRegularExpressionMatch m = colour.match(shown); m.hasMatch(); m = colour.match(shown))
                    shown.replace(m.capturedStart(), m.capturedLength(), "<" + m.captured(1) + m.captured(2) + " style=\"" + (m.captured(3).isEmpty() ? "color:" : "background-color:") + m.captured(4) + "\"");
                if (kind == "emote") shown = "* " + esc(name) + " " + shown;
            }
            content = "<span style=\"" + style + "\">" + shown + "</span>";
            if (B(r, "edited")) content += " <span style=\"color:" + muted + ";font-size:small\">[edited]</span>";
        }

        const QJsonObject reply = r.value("reply").toObject();
        if (!reply.isEmpty()) { /* what this message answers, quoted above it */
            QString snip = S(reply, "preview").simplified();
            if (snip.size() > 90) snip = snip.left(90) + "...";
            const QString who = S(reply, "sender").isEmpty() ? QStringLiteral("a message") : S(reply, "sender");
            content = "<a href=\"vc:goto/" + QString::fromLatin1(QUrl::toPercentEncoding(S(reply, "event_id"))) + (S(reply, "sender").isEmpty() ? "/" + QString::fromLatin1(QUrl::toPercentEncoding(eid)) : QString()) + "\"><i><span style=\"color:" + muted + "\">Replying to " + esc(who) + (snip.isEmpty() ? QString() : ": " + esc(snip)) + "</span></i></a><br>" + content;
        }
        const QJsonArray reactions = r.value("reactions").toArray();
        if (!eid.isEmpty() && !reactions.isEmpty()) {
            content += "<br>";
            for (const QJsonValue &rv : reactions) {
                const QJsonObject rx = rv.toObject();
                const QString key = S(rx, "key");
                const QString bg = B(rx, "mine") ? hex(pal.color(QPalette::Highlight)) : hex(pal.color(QPalette::AlternateBase));
                content += "<a href=\"vc:react/" + enc(eid) + "/" + enc(key) + "\" style=\"text-decoration:none\"><span style=\"background-color:" + bg + "\">&nbsp;" + esc(key) + " " +
                           QString::number(rx.value("count").toInt()) + "&nbsp;</span></a>&nbsp;";
            }
        }
        const int replies = r.value("thread_replies").toInt();
        if (replies > 0 && !thread_ && !eid.isEmpty())
            content += "<br><a href=\"vc:thread:" + esc(eid) + "\">" + QString::number(replies) + (replies == 1 ? " reply" : " replies") + "</a>";
        const QJsonObject pv = r.value("preview").toObject(); /* a card for the first link, when previews are on */
        if (!pv.isEmpty()) {
            QString img;
            const int across = int(96 * dpr);
            const QImage sc = drawn("pv", S(pv, "image_path"), [&](const QString &p) {
                const QImage got = loadFitted(p, QSize(across, 1 << 20));
                QImage s = got.isNull() ? got : got.scaledToWidth(across, Qt::SmoothTransformation);
                frameImage(s, base, dpr);
                s.setDevicePixelRatio(dpr);
                return s;
            });
            if (!sc.isNull()) {
                const QString key = "pv:" + eid;
                addRes(doc, resHash_, QUrl(key), sc);
                img = "<td valign=top><img src=\"" + key + "\" width=96></td>";
            }
            QString desc = S(pv, "description");
            if (desc.size() > 220) desc = desc.left(220) + "...";
            const QString url = S(pv, "url");
            content += "<br><table cellspacing=0 cellpadding=5 style=\"background-color:" + hex(pal.color(QPalette::AlternateBase)) + "\"><tr>" + img +
                       "<td valign=top><b><a href=\"" + esc(url) + "\">" + esc(S(pv, "title").isEmpty() ? url : S(pv, "title")) + "</a></b>" +
                       (S(pv, "site").isEmpty() ? QString() : "<br><span style=\"color:" + muted + "\">" + esc(S(pv, "site")) + "</span>") +
                       (desc.isEmpty() ? QString() : "<br>" + esc(desc)) + "</td></tr></table>";
        }

        /* What scrollToAnchor finds this message by. Qt's HTML import gives a pending anchor name to the first UTF-16 *unit* of the next text it inserts, on its own (qtextdocumentfragment.cpp,
           appendNodeText), so an anchor in front of an emoji cut its surrogate pair in two: two replacement characters, and in a run of flags the pairs after it were one off, which left
           no place to break the line (the view grew a horizontal scrollbar). The anchor therefore goes in front of text that is ours and starts with a plain character: the time of a
           message with a header, a non-breaking space in the otherwise empty left cell of one without. */
        const QString anchorName = eid.isEmpty() ? QString() : "ev" + QString::number(qHash(eid));
        const QString anchorBeforeTime = anchorName.isEmpty() ? QString() : "<a name=\"" + anchorName + "\"></a>";
        const QString anchorInBlank = anchorName.isEmpty() ? QString() : "<a name=\"" + anchorName + "\">&nbsp;</a>";
        const QString hl = !highlight_.isEmpty() && eid == highlight_ ? "bgcolor=\"" + hex(pal.color(QPalette::Highlight).lighter(isDark(pal) ? 100 : 150)) + "\"" : QString();
        if (!grouped) {
            html += QChar(2);
            chunkStart << rowEvents_.size();
            rowEvents_ << eid << eid;
            QString shieldHtml;
            const QJsonObject sh = r.value("shield").toObject();
            if (!sh.isEmpty()) shieldHtml = " <img src=\"shield:" + QString(S(sh, "level") == "red" ? "2" : "1") + "\" width=14 height=14 title=\"" + esc(S(sh, "text")) + "\">";
            const QString key = "av:" + sender;
            if (!haveAvatars.contains(key)) {
                addRes(doc, resHash_, QUrl(key), profilePixmap(S(r, "avatar_path"), sender, name, 32, dpr, pal).toImage());
                haveAvatars.insert(key);
            }
            html += "<tr><td width=40 rowspan=2 valign=top style=\"padding-top:6px\"><img src=\"" + key + "\" width=32 height=32></td>"
                    "<td " + hl + " style=\"padding-top:6px\"><b><font color=\"" + hex(nameColor(sender, pal)) + "\">" + esc(name) + "</font></b>" + shieldHtml + "</td>"
                    "<td " + hl + " align=right style=\"padding-top:6px\">" + anchorBeforeTime + "<font color=\"" + muted + "\">" + esc(when) + "</font></td></tr>"
                    "<tr><td " + hl + " colspan=2>" + content + "</td></tr>";
        } else {
            rowEvents_ << eid;
            html += "<tr><td width=40>" + anchorInBlank + "</td><td " + hl + " colspan=2" + (gapAbove ? " style=\"padding-top:8px\"" : "") + ">" + content + "</td></tr>";
        }
        const QJsonArray seen = r.value("seen_by").toArray();
        if (!seen.isEmpty()) {
            QStringList names;
            for (const QJsonValue &v : seen) names << v.toString();
            rowEvents_ << eid;
            html += "<tr><td></td><td colspan=2 align=right><font color=\"" + muted + "\" size=\"-1\">Read by " + esc(names.join(", ")) + "</font></td></tr>";
        }
    }

    if (!thread_ && lastShown < n) { /* the window stops short of the newest message: a link at the bottom (scrolling there adds messages too) */
        rowEvents_ << QString();
        html += "<tr><td colspan=3 align=center style=\"padding:6px\"><a href=\"vc:newer\">Show newer messages</a></td></tr>";
    }

    drawn_.setMaxCost(int(qBound<qint64>(qint64(kMinDrawnBytes), drawnBytes + drawnBytes / 4, qint64(kMaxDrawnBytes)))); /* what the next redraw needs plus a quarter, and at least some pictures of other rooms */

    /* One small table per message (rather than one table for the room): Qt lays that out about 40% faster, and it lets a change at the end of the
       room replace only the tables that changed instead of laying out everything again. */
    static const QString tableOpen = "<table cellspacing=0 cellpadding=0 width=100%>";
    QStringList chunks;
    QVector<int> starts;
    {
        const QStringList parts = html.split(QChar(2));
        if (!parts[0].isEmpty()) { chunks << tableOpen + parts[0] + "</table>"; starts << 0; }
        for (int k = 1; k < parts.size(); k++) { chunks << tableOpen + parts[k] + "</table>"; starts << chunkStart[k - 1]; }
    }
    chunkRowStart_ = starts;
    const QString fullHtml = "<html><body>" + chunks.join(QString()) + "</body></html>";
    const QString styleSig = pal.color(QPalette::Window).name() + pal.color(QPalette::Text).name();

    {   /* Nothing visible changed: leave the view alone. Redrawing identical content is what made the timeline flicker and move. */
        const QString state = fullHtml + QChar(1) + QString::number(resHash_) + QChar(1) + styleSig;
        if (state == lastState_ && !loadingMore_) { return; }
        lastState_ = state;
    }
    /* a picture whose size changed changes the height of its row: that needs the layout, not just a repaint */
    bool sizesSame = resSizes_.size() == resources_.size();
    for (auto it = resources_.cbegin(); sizesSame && it != resources_.cend(); ++it) {
        const auto old = resSizes_.constFind(it.key());
        if (old == resSizes_.cend() || old.value() != it.value().size()) sizesSame = false;
    }
    QHash<QUrl, QSize> nowSizes;
    for (auto it = resources_.cbegin(); it != resources_.cend(); ++it) nowSizes.insert(it.key(), it.value().size());
    resSizes_ = nowSizes;
    if (!lastChunks_.isEmpty() && chunks == lastChunks_ && lastStyle_ == styleSig && lastRes_ != resHash_ && !loadingMore_ && sizesSame) {
        lastRes_ = resHash_;
        view_->viewport()->update(); /* only pictures changed (an avatar arrived): sizes are fixed, the new pixels are simply drawn */
        return;
    }
    QScrollBar *sb = view_->verticalScrollBar();
    const bool stick = stick_;
    const int value = sb->value();

    /* Only the end of the room changed: cut the document back to the first table that differs and put the new tables in its place. */
    bool partial = false;
    int firstChanged = 0;
    if (!lastChunks_.isEmpty() && !chunks.isEmpty() && lastRes_ == resHash_ && lastStyle_ == styleSig && !loadingMore_) {
        while (firstChanged < lastChunks_.size() && firstChanged < chunks.size() && lastChunks_[firstChanged] == chunks[firstChanged]) firstChanged++;
        const QList<QTextFrame *> frames = doc->rootFrame()->childFrames();
        if (firstChanged >= 1 && frames.size() == lastChunks_.size()) {
            programmatic_ = true;
            QTextCursor cur(doc);
            cur.beginEditBlock();
            if (firstChanged < frames.size()) {
                cur.setPosition(frames[firstChanged]->firstPosition() - 1);
                cur.setPosition(doc->characterCount() - 1, QTextCursor::KeepAnchor);
                cur.removeSelectedText();
            } else cur.movePosition(QTextCursor::End);
            const QString suffix = chunks.mid(firstChanged).join(QString());
            if (!suffix.isEmpty()) cur.insertHtml(suffix);
            cur.endEditBlock();
            programmatic_ = false;
            partial = doc->rootFrame()->childFrames().size() == chunks.size();
        }
    }
    if (partial) {
        lastChunks_ = chunks; lastRes_ = resHash_; lastStyle_ = styleSig;
        if (stick) {
            programmatic_ = true; sb->setValue(sb->maximum()); programmatic_ = false;
            QTimer::singleShot(0, this, [this] { programmatic_ = true; view_->verticalScrollBar()->setValue(view_->verticalScrollBar()->maximum()); programmatic_ = false; });
        }
        QTimer::singleShot(0, this, [this] { placePlayer(); });
        return;
    }

    /* What the reader is looking at: the message at the top of the view and how far into it the view is. After the redraw that message is put back at
       the same place, so the text does not jump. */
    {   /* The new page is built and laid out in a document of its own and swapped in: setHtml on the shown document blanks it first (a flash). */
        QTextDocument *old = view_->document(), *nd = new QTextDocument(this);
        nd->setDocumentMargin(old->documentMargin());
        nd->setDefaultFont(old->defaultFont());
        nd->setDefaultStyleSheet(old->defaultStyleSheet());
        for (auto it = resources_.cbegin(); it != resources_.cend(); ++it) nd->addResource(QTextDocument::ImageResource, it.key(), it.value());
        nd->setPageSize(QSizeF(view_->viewport()->width(), -1));
        nd->setHtml(fullHtml);
        nd->setUndoRedoEnabled(false); /* setHtml switches it back on, and a document made here (unlike the browser's own) would record every edit: nothing ever undoes them, and each new message at the end of the room replaces tables, so the history grew by about 13 KB per message */
        (void)nd->size(); /* finishes the layout now */
        programmatic_ = true;
        view_->setDocument(nd);
        view_->verticalScrollBar()->setRange(0, qMax(0, int(nd->size().height()) - view_->viewport()->height()));
        programmatic_ = false;
        if (ownDoc_) delete ownDoc_;
        ownDoc_ = nd;
    }
    lastChunks_ = chunks; lastRes_ = resHash_; lastStyle_ = styleSig;
    auto restore = [this, stick, value, topEvent, delta] {
        QScrollBar *s = view_->verticalScrollBar();
        programmatic_ = true;
        if (stick) s->setValue(s->maximum());
        else if (!topEvent.isEmpty()) {
            const int y = anchorY(topEvent);
            s->setValue(y < 0 ? value : qBound(0, y + delta, s->maximum()));
        } else s->setValue(value);
        programmatic_ = false;
    };
    restore();
    QTimer::singleShot(0, this, restore); /* once more when the layout has settled */
    if (loadingMore_ && !loadingHistory_) QTimer::singleShot(50, this, [this] { loadingMore_ = false; });
    /* a short history that fits the window has no scrollbar to scroll up with: keep fetching until the window is full or the room's start is reached */
    QTimer::singleShot(250, this, [this] {
        if (!isVisible() || fitFetches_ >= 4 || thread_) return;
        if (view_->verticalScrollBar()->maximum() <= 0 && view_->document()->size().height() < view_->viewport()->height() && canLoadMore_ && !loadingHistory_) { fitFetches_++; loadingMore_ = true; emit olderRequested(); }
    });
    QTimer::singleShot(0, this, [this] { placePlayer(); });
}

void TimelineView::hideEvent(QHideEvent *e) { stopInline(); QWidget::hideEvent(e); }
void TimelineView::resizeEvent(QResizeEvent *e) { QWidget::resizeEvent(e); QTimer::singleShot(0, this, [this] { placePlayer(); }); }

/* A video is being downloaded: its card shows a turning arc until the file arrives (or fails). */
void TimelineView::setLoading(const QString &eventId)
{
    if (loadingId_ == eventId) return;
    loadingId_ = eventId;
    if (!spinner_) {
        spinner_ = new QTimer(this);
        spinner_->setInterval(120);
        connect(spinner_, &QTimer::timeout, this, [this] { spinStep_ = (spinStep_ + 1) % 12; if (isVisible()) refresh(); });
    }
    if (eventId.isEmpty()) spinner_->stop(); else { spinStep_ = 0; spinner_->start(); }
    if (isVisible()) refresh();
}

void TimelineView::stopInline()
{
    loadingId_.clear();
    if (spinner_) spinner_->stop();
    if (player_) { player_->deleteLater(); player_.clear(); }
    const bool was = !playingId_.isEmpty();
    playingId_.clear();
    playingPath_.clear();
    if (was && isVisible()) refresh(); /* the card goes back to its thumbnail */
}

/* A video file has arrived for this message: play it in place, over its card. */
void TimelineView::playFile(const QString &eventId, const QString &path)
{
    if (player_) { player_->deleteLater(); player_.clear(); }
    loadingId_.clear();
    if (spinner_) spinner_->stop();
    playingId_ = eventId;
    playingPath_ = path;
    render(); /* reserves the space for the player's control row */
    player_ = new VideoPlayer(view_->viewport());
    connect(player_, &VideoPlayer::closeRequested, this, [this] { stopInline(); });
    player_->open(path);
    player_->show();
    placePlayer();
}

/* The player sits over its card; the card is found through the text layout. */
void TimelineView::placePlayer()
{
    if (!player_ || playingId_.isEmpty()) return;
    QTextDocument *doc = view_->document();
    const QString href = "vc:vid:" + playingId_;
    for (QTextBlock b = doc->begin(); b.isValid(); b = b.next()) {
        for (QTextBlock::iterator it = b.begin(); !it.atEnd(); ++it) {
            QTextCharFormat cf = it.fragment().charFormat();
            if (!cf.isAnchor() || cf.anchorHref() != href) continue;
            QTextCursor cur(doc);
            cur.setPosition(it.fragment().position());
            const QPoint tl = view_->cursorRect(cur).topLeft();
            const QSize sz = playingSize_.isEmpty() ? QSize(360, 276) : playingSize_;
            player_->setGeometry(QRect(tl, sz));
            player_->show();
            player_->raise();
            return;
        }
    }
    player_->hide();
}

/* the message under the mouse: the top-level table has one row per rendered line, and rowEvents_ says which message each belongs to */
QString TimelineView::eventAt(const QPoint &pos) const
{
    QTextCursor c = view_->cursorForPosition(pos);
    QTextFrame *f = c.currentFrame();
    QTextFrame *root = view_->document()->rootFrame();
    while (f && f->parentFrame() && f->parentFrame() != root) f = f->parentFrame(); /* a table inside a message (poll, preview): the message's own table */
    QTextTable *tb = qobject_cast<QTextTable *>(f);
    if (!tb) return QString();
    const int k = root->childFrames().indexOf(tb);
    const int row = tb->cellAt(c.position()).row();
    if (k < 0 || k >= chunkRowStart_.size() || row < 0) return QString();
    const int at = chunkRowStart_[k] + row;
    return at < rowEvents_.size() ? rowEvents_[at] : QString();
}

void TimelineView::contextMenu(const QPoint &pos)
{
    const QString eid = eventAt(pos);
    const QJsonObject r = eid.isEmpty() ? QJsonObject() : row(eid);
    QMenu menu(this);
    if (view_->textCursor().hasSelection()) menu.addAction("Copy", view_, &QTextBrowser::copy);
    if (!r.isEmpty() && !B(r, "pending")) {
        const QString kind = S(r, "kind");
        const bool mine = B(r, "own");
        QMenu *react = menu.addMenu("React");
        for (const char *k : {"\xF0\x9F\x91\x8D", "\xE2\x9D\xA4\xEF\xB8\x8F", "\xF0\x9F\x98\x82", "\xF0\x9F\x8E\x89", "\xF0\x9F\x98\xAE", "\xF0\x9F\x98\xA2", "\xF0\x9F\x91\x8E"}) {
            const QString key = QString::fromUtf8(k);
            react->addAction(key, this, [this, eid, key] { emit reactRequested(eid, key); });
        }
        react->addAction("More...", this, [this, eid] { emit reactPickerRequested(eid); });
        menu.addAction("Reply", this, [this, eid] { emit replyRequested(eid); });
        if (!thread_) menu.addAction("Reply in thread", this, [this, eid] { emit threadRequested(eid); });
        if (mine && (kind == "text" || kind == "emote" || kind == "notice")) menu.addAction("Edit", this, [this, eid] { emit editRequested(eid); });
        const QString text = S(r, "body");
        if (!text.isEmpty()) menu.addAction("Copy message text", this, [text] { QApplication::clipboard()->setText(text); });
        const bool saved = bookmarks_.contains(eid);
        menu.addAction(saved ? "Remove from saved messages" : "Save message", this, [this, eid] { emit bookmarkRequested(eid); });
        const bool pinned = pinned_.contains(eid) || B(r, "pinned");
        menu.addAction(pinned ? "Unpin" : "Pin to the top of the room", this, [this, eid, pinned] { emit pinRequested(eid, !pinned); });
        if (kind == "image" || kind == "video" || kind == "audio") menu.addAction("Open", this, [this, eid] { emit openRequested(eid); });
        if (kind == "image" || kind == "video" || kind == "file" || kind == "audio") menu.addAction("Save as...", this, [this, eid] { emit saveRequested(eid); });
        if (kind != "poll") menu.addAction("Forward...", this, [this, eid] { emit forwardRequested(eid); });
        if (B(r, "edited")) menu.addAction("Edit history...", this, [this, eid] { emit historyRequested(eid); });
        menu.addAction("View source...", this, [this, eid] { emit sourceRequested(eid); });
        if (mine) {
            menu.addSeparator();
            menu.addAction("Delete", this, [this, eid] { emit deleteRequested(eid); });
        }
    }
    if (!menu.isEmpty()) menu.exec(view_->mapToGlobal(pos));
}

/* look of formatted messages (quotes, code); refreshed when the theme changes */
void TimelineView::applyStyle()
{
    const QPalette pal = view_->palette();
    const QString mutedc = hex(pal.color(QPalette::PlaceholderText)), codebg = hex(pal.color(QPalette::AlternateBase).darker(isDark(pal) ? 115 : 104));
    view_->document()->setDefaultStyleSheet(
        "blockquote { margin-left: 10px; margin-top: 2px; margin-bottom: 2px; color: " + mutedc + "; }"
        "pre { background-color: " + codebg + "; margin-top: 3px; margin-bottom: 3px; white-space: pre-wrap; }" /* a long line wraps: a view has one horizontal scrollbar for all its messages */
        "code { background-color: " + codebg + "; }"
        "h1, h2, h3 { margin-top: 4px; margin-bottom: 2px; }"
        "ul, ol { margin-top: 2px; margin-bottom: 2px; }"
        "table { border-color: " + mutedc + "; }");
}

/* Scrolls to a message and marks it for a moment (a search hit, a jump). */
void TimelineView::revealMessage(const QString &eventId)
{
    int at = -1;
    { const auto it = rowIndex_.constFind(eventId); if (it != rowIndex_.constEnd()) at = it.value(); }
    if (at < 0) { /* older than what is loaded: bring in history until it shows up */
        if (canLoadMore_ && !thread_ && !seekPending_) { seeking_ = eventId; seekTries_ = 0; seekPending_ = true; loadingMore_ = true; emit olderRequested(); }
        return;
    }
    stick_ = false;
    highlight_ = eventId;
    if (stale_ || at < hiddenLocal_ || at >= shownEnd_) { /* outside the document: a window around the message (never everything from it to the end of the room) */
        const int first = qMax(0, at - 30), last = qMin(int(rows_.size()), at + 150);
        windowStart_ = S(rows_[first].toObject(), "id");
        windowEnd_ = last < rows_.size() ? S(rows_[last].toObject(), "id") : QString();
    }
    if (!isVisible()) { pendingReveal_ = eventId; stale_ = true; return; }
    render();
    QTimer::singleShot(0, this, [this, eventId] {
        programmatic_ = true;
        view_->scrollToAnchor("ev" + QString::number(qHash(eventId)));
        programmatic_ = false;
    });
    QTimer::singleShot(2500, this, [this, eventId] { if (highlight_ == eventId) { highlight_.clear(); refresh(); } });
}

}
