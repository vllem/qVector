#include "qt/timelineview.h"
#include "qt/avatar.h"
#include "qt/emoji.h"
#include "qt/theme.h"
#include <QAbstractTextDocumentLayout>
#include <QApplication>
#include <QBuffer>
#include <QClipboard>
#include <QDateTime>
#include <QDesktopServices>
#include <QHBoxLayout>
#include <QImageReader>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QRegularExpression>
#include <QScrollBar>
#include <QTextBlock>
#include <QTextTable>
#include <QTimer>
#include <QVBoxLayout>

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

/* a picture from a file the engine downloaded (EXIF orientation applied), kept decoded */
const QImage *TimelineView::picture(const QString &path)
{
    if (path.isEmpty()) return nullptr;
    auto it = pictures_.constFind(path);
    if (it != pictures_.cend()) return it->isNull() ? nullptr : &*it;
    QImageReader rd(path);
    rd.setAutoTransform(true);
    pictures_.insert(path, rd.read());
    const QImage &img = pictures_[path];
    return img.isNull() ? nullptr : &img;
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

TimelineView::~TimelineView() { stopInline(); }

void TimelineView::reset()
{
    rows_ = QJsonArray();
    lastChunks_.clear(); rowEvents_.clear(); lastState_.clear();
    windowStart_.clear(); shownLimit_ = 100; stick_ = true; loadingMore_ = false; highlight_.clear();
    stopInline();
    canLoadMore_ = false; loadingHistory_ = false;
    programmatic_ = true;
    view_->setHtml(QString());
    programmatic_ = false;
}

void TimelineView::setRows(const QJsonArray &rows)
{
    rows_ = rows;
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
    for (const QJsonValue &v : rows_) if (v.toObject().value("id").toString() == eventId) return v.toObject();
    return QJsonObject();
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
    stick_ = sb->value() >= sb->maximum() - 4;
    fitFetches_ = 0;
    if (sb->value() <= 30) {
        if (hiddenLocal_ > 0) { shownLimit_ += 100; windowStart_.clear(); loadingMore_ = true; render(); }
        else if (canLoadMore_ && !loadingHistory_ && !thread_) { loadingMore_ = true; emit olderRequested(); }
    }
}

void TimelineView::onAnchor(const QUrl &u)
{
    const QString s = u.toString();
    auto part = [&](int n) { return QUrl::fromPercentEncoding(s.split('/').value(n).toUtf8()); };
    if (s == "vc:more") {
        if (hiddenLocal_ > 0) { shownLimit_ += 100; windowStart_.clear(); loadingMore_ = true; render(); }
        else { loadingMore_ = true; emit olderRequested(); }
    }
    else if (s.startsWith("vc:img:") || s.startsWith("vc:open:")) emit openRequested(QUrl::fromPercentEncoding(s.mid(s.indexOf(':', 3) + 1).toUtf8())); /* a gallery item is "<event>#<n>" */
    else if (s.startsWith("vc:save:")) emit saveRequested(QUrl::fromPercentEncoding(s.mid(8).toUtf8()));
    else if (s.startsWith("vc:text:")) emit textRequested(s.mid(8));
    else if (s.startsWith("vc:thread:")) emit threadRequested(s.mid(10));
    else if (s.startsWith("vc:goto/")) { /* vc:goto/<answered event>[/<the reply, when the original is not loaded>] */
        const QString target = part(1), reply = part(2);
        if (!reply.isEmpty()) emit loadReplyRequested(reply);
        revealMessage(target);
    }
    else if (s.startsWith("vc:vid:")) emit playRequested(s.mid(7));
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
    if (since < 150) {
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
    const QPalette pal = palette();
    const QString muted = hex(pal.color(QPalette::PlaceholderText)), link = hex(pal.color(QPalette::Link));
    QTextDocument *doc = view_->document();
    const qreal dpr = devicePixelRatioF();

    rowEvents_.clear();
    resHash_ = 0;
    resources_.clear();
    g_resStore = &resources_;
    applyStyle();
    for (int k = 0; k < 3; k++) addRes(doc, resHash_, QUrl(QString("shield:%1").arg(k)), shieldPixmap(k, 14, dpr).toImage());
    QString html; /* the rows; the document is a sequence of small tables, one per message, cut at the \x02 marks */
    QVector<int> chunkStart;
    const int n = rows_.size();
    /* Laying out a long room is what costs time (about 1 ms per message), so only the newest messages are in the document; the link or scrolling
       to the top adds more. The first message shown stays the same while new ones are added at the end, so the tables above never change. */
    int firstShown = 0;
    if (n > int(shownLimit_)) {
        int idx = -1;
        if (!windowStart_.isEmpty()) for (int i = 0; i < n; i++) if (S(rows_[i].toObject(), "id") == windowStart_) { idx = i; break; }
        if (idx < 0 || n - idx > int(shownLimit_) * 2) idx = n - int(shownLimit_);
        firstShown = idx;
        windowStart_ = S(rows_[idx].toObject(), "id");
    } else windowStart_.clear();
    hiddenLocal_ = firstShown;
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
    qint64 lastTs = 0;
    for (int i = firstShown; i < n; i++) {
        const QJsonObject r = rows_[i].toObject();
        const QString kind = S(r, "kind"), eid = S(r, "id"), sender = S(r, "sender_id"), name = S(r, "sender");
        const QString text = S(r, "body");
        const qint64 ts = qint64(r.value("ts").toDouble());
        const bool pending = B(r, "pending");
        const bool grouped = sender == lastSender && ts - lastTs < 5 * 60 * 1000 && ts >= lastTs;
        lastSender = sender; lastTs = ts;
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
            const QImage *img = picture(S(r, "image_path"));
            if (img) {
                const QString key = "img:" + eid;
                QImage scaled = img->scaled(QSize(480, 320) * dpr, Qt::KeepAspectRatio, Qt::SmoothTransformation);
                frameImage(scaled, pal.color(QPalette::Base), dpr);
                scaled.setDevicePixelRatio(dpr);
                addRes(doc, resHash_, QUrl(key), scaled);
                content = "<a href=\"vc:img:" + esc(eid) + "\"><img src=\"" + key + "\" width=" + QString::number(int(scaled.width() / dpr)) + "></a>";
            } else content = "<span style=\"color:" + muted + "\">Loading image...</span>";
            const QString fn = S(r, "file_name");
            if (!text.isEmpty() && text != "image" && text != fn) content += "<br>" + esc(text);
        } else if (kind == "sticker") { /* a picture without a frame, smaller than a photo */
            const QImage *img = picture(S(r, "image_path"));
            if (img) {
                const QString key = "stk:" + eid;
                QImage scaled = img->scaled(QSize(160, 160) * dpr, Qt::KeepAspectRatio, Qt::SmoothTransformation);
                scaled.setDevicePixelRatio(dpr);
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
                        const QImage *img = picture(S(it, "image_path"));
                        if (!img) { content += "<td width=" + QString::number(side) + " height=" + QString::number(side * 2 / 3) + " bgcolor=\"" + hex(pal.color(QPalette::AlternateBase)) + "\" align=center><span style=\"color:" + muted + "\">Loading...</span></td>"; continue; }
                        const QString key = "gal:" + eid + "#" + QString::number(it.value("index").toInt());
                        QImage t;
                        if (n == 1) t = img->scaled(QSize(total, 320) * dpr, Qt::KeepAspectRatio, Qt::SmoothTransformation);
                        else {
                            const QImage big = img->scaled(QSize(side, side) * dpr, Qt::KeepAspectRatioByExpanding, Qt::SmoothTransformation);
                            t = big.copy((big.width() - side * dpr) / 2, (big.height() - side * dpr) / 2, side * dpr, side * dpr);
                        }
                        frameImage(t, pal.color(QPalette::Base), dpr);
                        t.setDevicePixelRatio(dpr);
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
            if (!playingThis) {
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
            const QString fb = S(r, "html");
            if (kind == "text" && fb.isEmpty() && emojiOnly(text)) shown = "<span style=\"font-size:30pt\">" + esc(text) + "</span>";
            if (!fb.isEmpty() && kind != "undecryptable") { /* formatted text: sanitized HTML */
                shown = fb;
                for (const QJsonValue &ev : r.value("emoji").toArray()) { /* custom emoji: the downloaded picture at text height, else its shortcode */
                    const QJsonObject e = ev.toObject();
                    const QString mxc = S(e, "mxc");
                    const QImage *eimg = picture(S(e, "path"));
                    QString repl;
                    if (eimg) {
                        const QString key = "emo:" + mxc;
                        QImage scaled = eimg->scaledToHeight(int(22 * dpr), Qt::SmoothTransformation);
                        scaled.setDevicePixelRatio(dpr);
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
                shown.replace("<del>", "<s>").replace("</del>", "</s>"); /* Qt knows <s> */
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
            if (const QImage *pi = picture(S(pv, "image_path"))) {
                const QString key = "pv:" + eid;
                QImage sc = pi->scaledToWidth(int(96 * dpr), Qt::SmoothTransformation);
                frameImage(sc, pal.color(QPalette::Base), dpr);
                sc.setDevicePixelRatio(dpr);
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

        const QString anchor = eid.isEmpty() ? QString() : "<a name=\"ev" + QString::number(qHash(eid)) + "\"></a>";
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
                    "<td " + hl + " style=\"padding-top:6px\">" + anchor + "<b><font color=\"" + hex(nameColor(sender, pal)) + "\">" + esc(name) + "</font></b>" + shieldHtml + "</td>"
                    "<td " + hl + " align=right style=\"padding-top:6px\"><font color=\"" + muted + "\">" + esc(when) + "</font></td></tr>"
                    "<tr><td " + hl + " colspan=2>" + content + "</td></tr>";
        } else {
            rowEvents_ << eid;
            html += "<tr><td width=40></td><td " + hl + " colspan=2>" + anchor + content + "</td></tr>";
        }
        const QJsonArray seen = r.value("seen_by").toArray();
        if (!seen.isEmpty()) {
            QStringList names;
            for (const QJsonValue &v : seen) names << v.toString();
            rowEvents_ << eid;
            html += "<tr><td></td><td colspan=2 align=right><font color=\"" + muted + "\" size=\"-1\">Read by " + esc(names.join(", ")) + "</font></td></tr>";
        }
    }

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
    QString topEvent;
    int delta = 0;
    if (!stick && value > 0) {
        for (int y : {2, 14, 30, 60, 100}) { topEvent = eventAt(QPoint(24, y)); if (!topEvent.isEmpty()) break; }
        if (!topEvent.isEmpty()) delta = value - anchorY(topEvent);
    }
    {   /* The new page is built and laid out in a document of its own and swapped in: setHtml on the shown document blanks it first (a flash). */
        QTextDocument *old = view_->document(), *nd = new QTextDocument(this);
        nd->setDocumentMargin(old->documentMargin());
        nd->setDefaultFont(old->defaultFont());
        nd->setDefaultStyleSheet(old->defaultStyleSheet());
        for (auto it = resources_.cbegin(); it != resources_.cend(); ++it) nd->addResource(QTextDocument::ImageResource, it.key(), it.value());
        nd->setPageSize(QSizeF(view_->viewport()->width(), -1));
        nd->setHtml(fullHtml);
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

void TimelineView::stopInline()
{
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
        "pre { background-color: " + codebg + "; margin-top: 3px; margin-bottom: 3px; }"
        "code { background-color: " + codebg + "; }"
        "h1, h2, h3 { margin-top: 4px; margin-bottom: 2px; }"
        "ul, ol { margin-top: 2px; margin-bottom: 2px; }"
        "table { border-color: " + mutedc + "; }");
}

/* Scrolls to a message and marks it for a moment (a search hit, a jump). */
void TimelineView::revealMessage(const QString &eventId)
{
    int at = -1;
    for (int i = 0; i < rows_.size(); i++) if (S(rows_[i].toObject(), "id") == eventId) { at = i; break; }
    if (at < 0) { /* older than what is loaded: bring in history until it shows up */
        if (canLoadMore_ && !thread_ && !seekPending_) { seeking_ = eventId; seekTries_ = 0; seekPending_ = true; loadingMore_ = true; emit olderRequested(); }
        return;
    }
    stick_ = false;
    highlight_ = eventId;
    if (at < hiddenLocal_) { shownLimit_ = qMax(shownLimit_, size_t(rows_.size() - at)); windowStart_ = eventId; } /* still folded away above: show from there */
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
