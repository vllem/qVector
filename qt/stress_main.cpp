/* Torture tests for the Qt widgets: `qvector_stress` builds the real TimelineView, Sidebar and MemberList with absurd amounts of data and fails
   when a step is slower than a person would tolerate. Not part of the normal build:

       cmake --build build --target qvector_stress && QT_QPA_PLATFORM=offscreen build/qvector_stress

   Budgets are for a Release build; a Debug/ASan build gets 8x. Pass a name part to run only matching steps. */
#include <QApplication>
#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
#include <QFontDatabase>
#include <QFontInfo>
#include <QGuiApplication>
#include <QImage>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QListWidget>
#include <QRandomGenerator>
#include <QScreen>
#include <QScrollBar>
#include <QTemporaryDir>
#include <QTextBlock>
#include <QTextBoundaryFinder>
#include <QTextBrowser>
#include <QTextDocument>
#include <QTextFragment>
#include <QWheelEvent>
#include <cstdio>
#include <thread>
#include <vector>
#ifdef __GLIBC__
#include <malloc.h>
#endif
#include <QImageReader>
#include "qt/core.h"
#include "qt/imageload.h"
#include "qt/memberlist.h"
#include "qt/packpicker.h"
#include "qt/sidebar.h"
#include "qt/timelineview.h"

using namespace vc;

static int failures = 0;
static QString only;
static double excludedMs = 0; /* time a step spent building its own input, not the view's cost */
#ifdef QT_NO_DEBUG
static const double SLACK = 1;
#else
static const double SLACK = 8;
#endif

template <class F> static void step(const char *name, double budgetMs, F f)
{
    if (!only.isEmpty() && !QString(name).contains(only, Qt::CaseInsensitive)) return;
    QElapsedTimer t;
    t.start();
    excludedMs = 0;
    f();
    QApplication::processEvents(); /* the layout that was only scheduled */
    const double ms = t.nsecsElapsed() / 1e6 - excludedMs;
    const bool ok = ms <= budgetMs * SLACK;
    std::printf("%-58s %9.1f ms  (budget %.0f)  %s\n", name, ms, budgetMs * SLACK, ok ? "ok" : "TOO SLOW");
    std::fflush(stdout);
    if (!ok) failures++;
}

/* The view merges redraws that come within 150 ms of each other, so a check of what it shows waits for the deferred one. */
static void settle()
{
    QElapsedTimer w;
    w.start();
    while (w.elapsed() < 300) QApplication::processEvents(QEventLoop::AllEvents, 20);
}

/* Resident anonymous memory of this process in KiB: the heap, image buffers and stacks, not the file-backed pages of the libraries. -1 without /proc. */
static long rssAnonKiB()
{
    QFile f("/proc/self/status");
    if (!f.open(QIODevice::ReadOnly)) return -1;
    for (const QByteArray &l : f.readAll().split('\n')) if (l.startsWith("RssAnon:")) return l.mid(8).trimmed().split(' ').first().toLong();
    return -1;
}

/* What a view really holds once it has settled: pending redraws done, free heap pages given back to the system. */
static long freeHeapKiB = 0; /* what glibc held on to, free, just before the last malloc_trim */
static long heldKiB()
{
    settle();
    const long pre = rssAnonKiB();
#ifdef __GLIBC__
    malloc_trim(0);
#endif
    const long post = rssAnonKiB();
    freeHeapKiB = pre - post;
    return post;
}

/* Like step(), for memory: `f` may grow the process by `budgetKiB` at most (resident anonymous memory after everything has settled). The budgets are
   for a screen at scale 1; pictures are drawn at the device's pixels, so at scale 2 (QT_SCALE_FACTOR=2) they have four times the bytes. */
template <class F> static void memstep(const char *name, long budgetKiB, F f, long maxFreeHeapKiB = -1)
{
    if (!only.isEmpty() && !QString(name).contains(only, Qt::CaseInsensitive)) return;
    const double scale = qMax(1.0, QGuiApplication::primaryScreen()->devicePixelRatio());
    budgetKiB = long(budgetKiB * scale * scale);
    const long before = heldKiB();
    f();
    const long grew = heldKiB() - before;
    const bool ok = before < 0 || grew <= budgetKiB;
    std::printf("%-58s %+8ld KiB  (budget %ld)  %s   [free heap before trim: %ld KiB]\n", name, grew, budgetKiB, ok ? "ok" : "TOO MUCH MEMORY", freeHeapKiB);
    std::fflush(stdout);
    if (!ok) failures++;
    /* with `maxFreeHeapKiB`: what glibc may keep free between two trims. glibc raises the size above which a block gets a mapping of its own to the biggest block
       it has freed, so the buffers of a decoded picture then stay in the heap when they are freed (25 MB were held after the pictures step): the timeline gives
       the heap back itself when it has decoded pictures (giveBackFreedMemory). */
    if (before >= 0 && maxFreeHeapKiB >= 0 && freeHeapKiB > maxFreeHeapKiB) { std::printf("    glibc held %ld KiB of free memory before the trim (limit %ld)\n", freeHeapKiB, maxFreeHeapKiB); failures++; }
}

/* a row as the engine sends it: a plain text message from a person whose picture is known (the engine leaves out every field that has its default), as JSON
   text parsed by Qt, which is what the view really holds per message */
static QJsonArray engineRows(int n)
{
    QJsonArray src;
    for (int i = 0; i < n; i++)
        src.append(QJsonObject{{"id", QString("$event%1abcdefghijklmnopqrstuvwxyz0123456789:example.org").arg(i)}, {"sender", "Alice Example"}, {"sender_id", "@alice:example.org"},
                               {"body", QString("message %1: the quick brown fox jumps over the lazy dog, again and again until the line wraps around the window").arg(i)}, {"kind", "text"},
                               {"time", "12:34"}, {"ts", 1700000000000.0 + i},
                               {"avatar_path", "/home/user/.local/share/vector/vector/account/media/avatars/mxcexampleorgabcdefghijklmnop.img"}});
    return QJsonDocument::fromJson(QJsonDocument(src).toJson(QJsonDocument::Compact)).array();
}

/* A photo-like picture (a gradient with a little noise), so the JPEG and PNG files come out about as big as real ones. */
static QImage photo(int w, int h, quint32 seed)
{
    QImage img(w, h, QImage::Format_RGB32);
    QRandomGenerator rg(seed);
    for (int y = 0; y < h; y++) {
        QRgb *line = reinterpret_cast<QRgb *>(img.scanLine(y));
        for (int x = 0; x < w; x++) {
            const quint32 n = rg.generate();
            line[x] = qRgb(int((x * 255LL / w + (n & 15)) & 255), int((y * 255LL / h + ((n >> 4) & 15)) & 255), int(((x + y) * 255LL / (w + h) + ((n >> 8) & 15)) & 255));
        }
    }
    return img;
}

static QJsonObject message(int i, const QString &body, const QString &html = QString())
{
    static const char *const who[] = {"zach", "ben", "matt", "ruben", "kab"};
    const QString w = who[i % 5];
    QJsonObject o{{"id", QString("$e%1").arg(i)}, {"sender", w}, {"sender_id", "@" + w + ":hs"}, {"body", body}, {"html", html}, {"kind", "text"},
                  {"time", "12:00"}, {"ts", 1000000.0 + i}, {"own", i % 9 == 0}, {"edited", false}, {"pending", false}, {"reactions", QJsonArray()},
                  {"seen_by", QJsonArray()}, {"thread_replies", 0}, {"pinned", false}};
    return o;
}

static QJsonArray messages(int n, int from = 0)
{
    QJsonArray a;
    for (int i = from; i < from + n; i++)
        a.append(message(i, QString("message %1: the quick brown fox jumps over the lazy dog, again and again until the line wraps around the window").arg(i)));
    return a;
}

static QString nest(int depth)
{
    QString s;
    for (int i = 0; i < depth; i++) s += "<blockquote>";
    s += "deep";
    for (int i = 0; i < depth; i++) s += "</blockquote>";
    return s;
}

int main(int argc, char **argv)
{
#ifdef __GLIBC__
    mallopt(M_ARENA_MAX, 2); /* as in the app (qt/main.cpp): the memory steps measure what the app does */
#endif
    QApplication app(argc, argv);
    QApplication::setApplicationName("vector"); /* as in the app: the widgets read their QSettings under these names */
    QApplication::setOrganizationName("vector");
    if (argc > 1) only = argv[1];

    TimelineView tl;
    tl.resize(900, 600);
    tl.show();
    app.processEvents();

    step("timeline: open a room of 50 000 messages", 1300, [&] { tl.setRows(messages(50000)); });
    step("timeline: scroll to the top and back 20 times", 800, [&] {
        auto *bar = tl.findChild<QTextBrowser *>()->verticalScrollBar();
        for (int i = 0; i < 20; i++) { bar->setValue(bar->minimum()); app.processEvents(); bar->setValue(bar->maximum()); app.processEvents(); }
    });
    step("timeline: 300 new messages arriving one by one in 50k", 3000, [&] {
        QJsonArray rows = messages(50000);
        qint64 build = 0; /* the harness copy of the array (QJsonArray detaches on append) is not the view's cost */
        for (int i = 0; i < 300; i++) { QElapsedTimer b; b.start(); rows.append(message(50000 + i, "new")); build += b.nsecsElapsed(); tl.setRows(rows); app.processEvents(); }
        excludedMs = build / 1e6;
    });
    step("timeline: 20 000 event-id look-ups (reply, reaction, jump)", 700, [&] {
        tl.setRows(messages(20000));
        for (int i = 0; i < 20000; i += 1) tl.row(QString("$e%1").arg(19999 - i % 40)); /* near the end, like real traffic */
    });
    step("timeline: look up ids at the very start 2000 times", 300, [&] { for (int i = 0; i < 2000; i++) tl.row("$e0"); });
    /* Jumping lays out the messages around the target only (about 1 ms each): the document is a window of at most kWindowMax messages. */
    step("timeline: reveal a message 1500 rows back in 20k rows", 2500, [&] { tl.revealMessage("$e18500"); });

    tl.reset();
    step("timeline: a 6000-character message of sentences and a link", 300, [&] {
        QString b; for (int i = 0; i < 60; i++) b += "This is sentence number " + QString::number(i) + ", written the way people write.Some run on. See https://example.org/a/b?c=d for more (really).\n";
        tl.setRows(QJsonArray{message(1, b)});
    });
    tl.reset();
    step("timeline: one 5 MB message body", 1500, [&] { tl.setRows(QJsonArray{message(1, QString(5000000, 'a'))}); });
    step("timeline: one 2 MB message of words", 1500, [&] { tl.setRows(QJsonArray{message(1, QString("lorem ipsum dolor sit amet ").repeated(80000))}); });
    step("timeline: 200 lines of 20 000 unbreakable characters", 1500, [&] {
        QJsonArray a;
        for (int i = 0; i < 200; i++) a.append(message(i, QString(20000, QChar('x'))));
        tl.setRows(a);
    });
    step("timeline: blockquotes nested 2000 deep", 1500, [&] { tl.setRows(QJsonArray{message(1, "deep", nest(2000))}); });
    step("timeline: 5000 emoji (ZWJ, flags, skin tones) in 100 rows", 1500, [&] {
        QJsonArray a;
        const QString e = QString::fromUtf8("\xf0\x9f\x91\xa8\xe2\x80\x8d\xf0\x9f\x91\xa9\xe2\x80\x8d\xf0\x9f\x91\xa7\xf0\x9f\x87\xa9\xf0\x9f\x87\xaa\xf0\x9f\x91\x8d\xf0\x9f\x8f\xbd");
        for (int i = 0; i < 100; i++) a.append(message(i, e.repeated(50)));
        tl.setRows(a);
    });
    step("timeline: right-to-left, combining marks and NULs", 800, [&] {
        QJsonArray a;
        for (int i = 0; i < 100; i++) a.append(message(i, QString::fromUtf8("\xd7\xa9\xd7\x9c\xd7\x95\xd7\x9d \xd8\xb3\xd9\x84\xd8\xa7\xd9\x85 e\xcc\x81\xcc\x82\xcc\x83\xcc\x84\xcc\x85 ") .repeated(20) + QString(QChar(0)) + "end"));
        tl.setRows(a);
    });
    step("timeline: 100 messages with 500 reactions each", 5000, [&] {
        QJsonArray a;
        for (int i = 0; i < 100; i++) {
            QJsonObject m = message(i, "reacted");
            QJsonArray r;
            for (int k = 0; k < 500; k++) r.append(QJsonObject{{"key", QString("k%1").arg(k)}, {"count", k + 1}, {"mine", k % 7 == 0}, {"senders", QJsonArray{"a", "b"}}});
            m["reactions"] = r;
            a.append(m);
        }
        tl.setRows(a);
    });
    step("timeline: 300 pictures whose files are missing", 1500, [&] {
        QJsonArray a;
        for (int i = 0; i < 300; i++) { QJsonObject m = message(i, "photo.png"); m["kind"] = "image"; m["image_path"] = QString("/nonexistent/%1.png").arg(i); m["image_w"] = 4000; m["image_h"] = 3000; a.append(m); }
        tl.setRows(a);
    });
    step("timeline: 100 messages that are each 100 links", 1500, [&] {
        QJsonArray a;
        for (int i = 0; i < 100; i++) { QString b; for (int k = 0; k < 100; k++) b += QString("https://example.org/p/%1/%2 ").arg(i).arg(k); a.append(message(i, b)); }
        tl.setRows(a);
    });
    step("timeline: resize the window 100 times with 100 rows", 3000, [&] {
        tl.setRows(messages(100));
        for (int i = 0; i < 100; i++) { tl.resize(500 + (i * 37) % 700, 400 + (i * 53) % 400); app.processEvents(); }
    });
    step("timeline: older messages arriving leave the reader where they were", 2000, [&] {
        tl.reset();
        tl.setHistoryState(true, false);
        tl.setRows(messages(100, 1000));
        settle();
        auto *br = tl.findChild<QTextBrowser *>();
        QWheelEvent w(QPointF(50, 50), QPointF(50, 50), QPoint(), QPoint(0, 120), Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
        QApplication::sendEvent(br->viewport(), &w); /* the reader's own input, or the view ignores the scrollbar */
        br->verticalScrollBar()->setValue(0);
        QApplication::processEvents();
        auto top = [&] { return tl.eventAt(QPoint(24, 100)); }; /* what is shown 100 px down must stay there */
        const QString before = top();
        tl.setHistoryState(true, true);
        settle();
        tl.setHistoryState(true, false);
        tl.setRows(messages(200, 900)); /* 100 older messages arrive: $e900..$e999 */
        settle();
        const QString after = top();
        std::printf("    top before %s, after %s (value %d of %d)\n", qPrintable(before), qPrintable(after), br->verticalScrollBar()->value(), br->verticalScrollBar()->maximum());
        if (after != before) failures++;
    });
    /* A new message at the end of the room edits the shown document through a cursor. If that document keeps an undo history, every edit keeps the
       text it replaced for as long as the document lives: nothing ever undoes it, so the history must stay empty. */
    step("timeline: 30 new messages one by one leave no undo history", 9000, [&] {
        tl.reset();
        QJsonArray rows = messages(100);
        tl.setRows(rows);
        settle();
        auto pause = [&](int ms) { QElapsedTimer w; w.start(); while (w.elapsed() < ms) QApplication::processEvents(QEventLoop::AllEvents, 10); };
        for (int i = 0; i < 30; i++) { rows.append(message(100 + i, "arriving"));  tl.setRows(rows); pause(170); } /* the view merges redraws within 150 ms */
        const int steps = tl.findChild<QTextBrowser *>()->document()->availableUndoSteps();
        std::printf("    undo steps kept by the document: %d\n", steps);
        if (steps != 0) failures++;
    });
    step("timeline: the same rows set 500 times (must be skipped)", 300, [&] { const QJsonArray rows = messages(100); for (int i = 0; i < 500; i++) tl.setRows(rows); });
    /* The document is a window on the room's messages and never holds more than kWindowMax of them: scrolling up through a long room adds messages above
       and drops the newest, scrolling back down adds messages below and drops the oldest, and the reader stays where they were. */
    auto readerInput = [](QTextBrowser *br) {
        QWheelEvent w(QPointF(50, 50), QPointF(50, 50), QPoint(), QPoint(0, 120), Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
        QApplication::sendEvent(br->viewport(), &w); /* the reader's own input, or the view ignores the scrollbar */
    };
    auto visible = [](TimelineView &v, QTextBrowser *br) { QSet<QString> seen; for (int y = 2; y < br->viewport()->height(); y += 8) { const QString e = v.eventAt(QPoint(24, y)); if (!e.isEmpty()) seen.insert(e); } return seen; };
    /* Each page lays out the whole window again (300 messages, about 1 ms each, 250 to 350 ms on this machine; the budgets are 40 and 38 pages at 500 ms): before the window had
       a limit the same 40 pages took 100 seconds, laying out 100, 200, ... 4 100 messages, and held 4 100 messages in the document. */
    step("timeline: 40 pages scrolled up through 20 000 rows stay within 300", 20000, [&] {
        tl.reset();
        tl.setRows(messages(20000));
        settle();
        auto *br = tl.findChild<QTextBrowser *>();
        int widest = 0, moved = 0;
        for (int i = 0; i < 40; i++) {
            const QString first = QString("$e%1").arg(tl.shownFirst()); /* the oldest message of the document: the reader reaches it, and it must stay in view when more come in above it */
            readerInput(br);
            br->verticalScrollBar()->setValue(0);
            QApplication::processEvents();
            widest = qMax(widest, tl.shownEnd() - tl.shownFirst());
            if (!visible(tl, br).contains(first)) moved++;
        }
        std::printf("    window now [%d, %d) of %lld, widest %d, reader moved %d times\n", tl.shownFirst(), tl.shownEnd(), (long long)tl.rows().size(), widest, moved);
        if (widest > TimelineView::kWindowMax || moved != 0 || tl.shownFirst() != 20000 - 100 - 40 * 100 || tl.shownEnd() - tl.shownFirst() != TimelineView::kWindowMax) failures++;
    });
    step("timeline: scrolled back down, the oldest go and the newest come", 19000, [&] {
        auto *br = tl.findChild<QTextBrowser *>();
        int widest = 0, moved = 0, pages = 0;
        while (tl.shownEnd() < tl.rows().size() && pages++ < 100) {
            const QString lastOld = QString("$e%1").arg(tl.shownEnd() - 1); /* the newest message of the document: the reader reaches it, and it must stay in view when more come in below it */
            readerInput(br);
            br->verticalScrollBar()->setValue(br->verticalScrollBar()->maximum());
            QApplication::processEvents();
            widest = qMax(widest, tl.shownEnd() - tl.shownFirst());
            if (!visible(tl, br).contains(lastOld)) moved++;
        }
        std::printf("    window now [%d, %d) of %lld after %d pages, widest %d, reader moved %d times\n", tl.shownFirst(), tl.shownEnd(), (long long)tl.rows().size(), pages, widest, moved);
        if (widest > TimelineView::kWindowMax || tl.shownEnd() != tl.rows().size() || moved != 0) failures++;
    });
    step("timeline: new messages arriving while the window stops short of the end change nothing", 3000, [&] {
        tl.reset();
        QJsonArray rows = messages(1000);
        tl.setRows(rows);
        settle();
        auto *br = tl.findChild<QTextBrowser *>();
        for (int i = 0; i < 3; i++) { readerInput(br); br->verticalScrollBar()->setValue(0); QApplication::processEvents(); } /* 600 rows: detached from the end */
        const int first = tl.shownFirst(), end = tl.shownEnd();
        const QString before = br->toPlainText();
        for (int i = 0; i < 20; i++) rows.append(message(1000 + i, "arriving"));
        tl.setRows(rows);
        settle();
        const bool same = br->toPlainText() == before && tl.shownFirst() == first && tl.shownEnd() == end;
        std::printf("    window [%d, %d) before and after 20 new messages: [%d, %d), text %s\n", first, end, tl.shownFirst(), tl.shownEnd(), same ? "the same" : "CHANGED");
        if (!same || end >= 1020 || !br->toPlainText().contains("Show newer messages")) failures++;
        tl.stickToBottom(); /* the reader sends a message: follow the end again */
        settle();
        if (tl.shownEnd() != 1020 || br->toPlainText().contains("Show newer messages")) { std::printf("    did not follow the end again: [%d, %d)\n", tl.shownFirst(), tl.shownEnd()); failures++; }
    });
    step("timeline: the link and sending bring a window that stops short of the end back to the newest", 4000, [&] {
        for (int how = 0; how < 2; how++) {
            tl.reset();
            tl.setRows(messages(1000));
            settle();
            auto *br = tl.findChild<QTextBrowser *>();
            for (int i = 0; i < 3; i++) { readerInput(br); br->verticalScrollBar()->setValue(0); QApplication::processEvents(); }
            const bool detached = tl.shownEnd() < 1000 && br->toPlainText().contains("Show newer messages");
            if (how == 0) emit br->anchorClicked(QUrl("vc:newer")); else tl.stickToBottom();
            settle();
            const bool back = tl.shownEnd() == 1000 && !br->toPlainText().contains("Show newer messages");
            std::printf("    %s: the window stopped short of the end: %s, the newest messages again: %s\n", how == 0 ? "the link" : "sending", detached ? "yes" : "NO", back ? "yes" : "NO");
            if (!detached || !back) failures++;
        }
    });
    step("timeline: reveal the first message of 20 000 rows and then the last", 3000, [&] {
        tl.reset();
        tl.setRows(messages(20000));
        settle();
        tl.revealMessage("$e5");
        settle();
        const bool near = tl.shownFirst() == 0 && tl.shownEnd() <= TimelineView::kWindowMax;
        std::printf("    after $e5: [%d, %d)\n", tl.shownFirst(), tl.shownEnd());
        tl.revealMessage("$e19990");
        settle();
        std::printf("    after $e19990: [%d, %d)\n", tl.shownFirst(), tl.shownEnd());
        if (!near || tl.shownEnd() != 20000 || tl.shownEnd() - tl.shownFirst() > TimelineView::kWindowMax) failures++;
    });
    /* Spec conformance of the renderer: a spoiler hides its text until clicked, colours become CSS (Client-Server API: Spoiler messages, formatted text). */
    step("timeline: spoilers stay hidden until clicked; colours are applied", 1500, [&] {
        tl.reset();
        tl.setRows(QJsonArray{message(1, "x", "The answer is <span data-mx-spoiler=\"plot\">forty-two</span> and <font data-mx-color=\"#ff0000\">red</font>")});
        settle();
        auto *b = tl.findChild<QTextBrowser *>();
        if (b->toPlainText().contains("forty-two") || !b->toPlainText().contains("plot")) { std::printf("  spoiler text is visible or its reason is missing\n"); failures++; }
        if (!b->toHtml().contains("color:#ff0000")) { std::printf("  data-mx-color not applied\n"); failures++; }
        emit b->anchorClicked(QUrl("vc:spoiler/$e1/0"));
        settle();
        if (!b->toPlainText().contains("forty-two")) { std::printf("  spoiler did not open\n"); failures++; }
    });
    step("timeline: markup inside a spoiler cannot end it early", 1500, [&] {
        tl.reset();
        tl.setRows(QJsonArray{message(2, "x", "<span data-mx-spoiler><span data-mx-color=\"#ff0000\">red</span> secret <span>deep <span>deeper</span> still</span> tail</span> after")});
        settle();
        const QString t = tl.findChild<QTextBrowser *>()->toPlainText();
        for (const char *w : {"red", "secret", "deeper", "still", "tail"}) if (t.contains(w)) { std::printf("  '%s' leaks out of the spoiler\n", w); failures++; }
        if (!t.contains("after")) { std::printf("  text after the spoiler is lost\n"); failures++; }
    });
    step("timeline: a numbered list keeps its start number (spec v1.18)", 1500, [&] {
        tl.reset();
        tl.setRows(QJsonArray{message(1, "x", "<ol start=\"5\"><li>five</li><li>six</li></ol>")});
        settle();
        if (!tl.findChild<QTextBrowser *>()->toHtml().contains("start=\"5\"")) { std::printf("  the list start number is lost\n"); failures++; }
    });
    step("timeline: malformed rows (missing and wrong-typed fields)", 300, [&] {
        QJsonArray a;
        a.append(QJsonObject());
        a.append(QJsonObject{{"id", 5}, {"body", QJsonArray{1, 2}}, {"reactions", "oops"}, {"reply", 7}, {"ts", "later"}});
        a.append(QJsonValue());
        a.append(QJsonObject{{"id", "$x"}, {"kind", "poll"}, {"poll", QJsonObject()}, {"gallery", QJsonArray{QJsonObject()}}});
        tl.setRows(a);
    });

    /* Laying out an emoji takes about a millisecond (Qt asks the colour font for the bitmap of every glyph it places), so a message of thousands of them froze the window for
       seconds on every redraw of the room: only the first 400 are laid out, and the message says it was cut. */
    step("timeline: a message of 5 000 flags is cut at 400 emoji and lays out fast", 1200, [&] {
        TimelineView e;
        e.resize(900, 600);
        e.show();
        const QString il = QString::fromUtf8("\xF0\x9F\x87\xAE\xF0\x9F\x87\xB1");
        QString many; for (int i = 0; i < 5000; i++) many += il;
        e.setRows(QJsonArray{message(1, many)});
        settle();
        const QString shown = e.findChild<QTextBrowser *>()->toPlainText();
        const bool cut = shown.contains("message cut, 20000 characters in all") && shown.count(QString::fromUtf8("\xF0\x9F\x87\xAE")) == 200; /* 400 emoji: 200 flags, none of them half */
        std::printf("    the note is there and %d flags are shown: %s\n", int(shown.count(QString::fromUtf8("\xF0\x9F\x87\xAE"))), cut ? "ok" : "WRONG");
        if (!cut) failures++;
        QString few; for (int i = 0; i < 150; i++) few += il; /* 300 emoji: shown whole */
        e.reset();
        e.setRows(QJsonArray{message(2, few)});
        settle();
        const QString whole = e.findChild<QTextBrowser *>()->toPlainText();
        if (whole.contains("message cut") || whole.count(QString::fromUtf8("\xF0\x9F\x87\xAE")) != 150) { std::printf("    WRONG: 150 flags are not shown whole\n"); failures++; }
    });

    /* No message makes the view wider than the window. A view has one horizontal scrollbar for all its messages, and one line that cannot wrap (a code block, a picture
       that says it is 100 000 pixels wide) gave a whole room a scrollbar tens of thousands of pixels long. Runs of emoji must wrap too, and must not be split inside a flag
       (a pair of regional indicators): a split one shows as a lone letter and every flag after it as the neighbouring country's. */
    step("timeline: no kind of message widens the view", 3000, [&] {
        TimelineView wide;
        wide.resize(900, 600);
        { /* the font as the app sets it (main.cpp ensureEmojiFont): the normal font first, a colour emoji font behind it */
            QFont f = QApplication::font();
            const QStringList have = QFontDatabase::families();
            for (const char *want : {"Noto Color Emoji", "Twemoji Mozilla", "Twitter Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"}) if (have.contains(want)) { f.setFamilies({QFontInfo(f).family(), want}); break; }
            wide.setFont(f);
        }
        wide.show();
        auto *bar = wide.findChild<QTextBrowser *>()->horizontalScrollBar();
        auto run = [](int n, const QString &unit) { QString t; while (t.size() < n) t += unit; return t.left(n); };
        const QString il = QString::fromUtf8("\xF0\x9F\x87\xAE\xF0\x9F\x87\xB1"), ri = QString::fromUtf8("\xF0\x9F\x87\xAE"); /* the Israeli flag and its first regional indicator */
        struct Case { QString name; QJsonObject row; bool flags = false; };
        QList<Case> cases;
        auto plain = [&](const QString &n, const QString &t, bool flags = false) { cases.append({n, message(1, t), flags}); };
        auto html = [&](const QString &n, const QString &h) { cases.append({n, message(1, "x", h), false}); };
        auto field = [&](const QString &n, const QString &k, const QJsonValue &v) { QJsonObject r = message(1, "hi"); r[k] = v; cases.append({n, r, false}); };
        plain("20000 letters", run(20000, "a")); plain("20000 binary digits", run(20000, "01101110")); plain("base64", run(5000, "QUJDREVGR0hJSktMTU5PUA+/=="));
        plain("3000 grinning faces", run(6000, QString::fromUtf8("\xF0\x9F\x98\x80"))); plain("CJK", run(3000, QString::fromUtf8("\xE6\xBC\xA2\xE5\xAD\x97")));
        plain("long address", "https://example.org/path?" + run(3000, "a=1&b=2&")); plain("run of equals signs", run(3000, "="));
        plain("497 Israeli flags", run(497 * 4, il), true); plain("8000 Israeli flags", run(8000 * 4, il), true); plain("flags and a half one", run(200 * 4, il) + ri, true);
        html("code block with one 3000-character line", "<pre><code>" + run(3000, "abcdefghij") + "</code></pre>");
        html("code block of 100 lines of 300 characters", "<pre><code>" + run(30000, run(300, "z") + "\n") + "</code></pre>");
        html("code block of flags", "<pre><code>" + run(300 * 4, il) + "</code></pre>");
        html("long link text", "<a href=\"https://example.org/\">" + run(3000, "a") + "</a>"); html("inline code", "see <code>" + run(3000, "a") + "</code> here");
        { QString t = "<table>"; for (int r = 0; r < 3; r++) { t += "<tr>"; for (int c = 0; c < 40; c++) t += "<td>cell number " + QString::number(c) + "</td>"; t += "</tr>"; } html("table of 40 columns", t + "</table>"); }
        { QString t = "<table><tr>"; for (int c = 0; c < 3; c++) t += "<td>" + run(400, "tok") + "</td>"; html("table of cells with 400-character words", t + "</tr></table>"); }
        html("quotation", "<blockquote>" + run(3000, "a") + "</blockquote>"); html("list item", "<ul><li>" + run(3000, "a") + "</li></ul>"); html("heading", "<h2>" + run(3000, "a") + "</h2>");
        html("emphasis", "<strong><em>" + run(3000, "a") + "</em></strong>"); html("coloured text", "<font data-mx-color=\"#ff0000\">" + run(3000, "a") + "</font>");
        html("spoiler", "<span data-mx-spoiler>" + run(3000, "a") + "</span>"); html("paragraphs", "<p>" + run(5000, "a b") + "</p><p>" + run(5000, "a") + "</p>");
        html("picture that says it is 100 000 pixels wide", "<img src=\"mxc://hs/e\" width=\"100000\" height=\"32\">");
        html("picture of somebody else with an enormous height", "<img src=\"https://example.org/x.png\" width=\"50\" height=\"900000\">");
        field("sender with a 500-character name", "sender", run(500, "N")); field("file with a 3000-character name", "file_name", run(3000, "f"));
        field("reply to a 3000-character message", "reply", QJsonObject{{"event_id", "$x"}, {"sender", "zach"}, {"preview", run(3000, "p")}});
        field("reaction with a 3000-character key", "reactions", QJsonArray{QJsonObject{{"key", run(3000, "r")}, {"count", 1}, {"mine", false}}});
        { QJsonObject r = message(1, run(3000, "c")); r["kind"] = "image"; r["file_name"] = run(3000, "i") + ".jpg"; cases.append({"picture with a 3000-character caption", r, false}); }
        int wideCases = 0, splitFlags = 0;
        for (const Case &c : cases) {
            QElapsedTimer one; one.start();
            wide.reset();
            wide.setRows(QJsonArray{message(0, "before"), c.row, message(2, "after")});
            QElapsedTimer pause; pause.start(); while (pause.elapsed() < 170) QApplication::processEvents(QEventLoop::AllEvents, 10); /* the view merges redraws within 150 ms */
            excludedMs += pause.elapsed();
            if (one.elapsed() - pause.elapsed() > 400) std::printf("    (%lld ms: %s)\n", (long long)(one.elapsed() - pause.elapsed()), qPrintable(c.name));
            if (bar->maximum() > 0) { std::printf("    WIDER THAN THE WINDOW by %d px: %s\n", bar->maximum(), qPrintable(c.name)); wideCases++; }
            if (c.flags) { /* red is the neighbours' flags (Liechtenstein: L, I); the Israeli one has none (only the sender's name is dark red, at the top) */
                const QImage img = wide.grab().toImage();
                int red = 0;
                for (int y = 100; y < img.height() - 100; y++) for (int x = 40; x < img.width() - 20; x++) { const QRgb px = img.pixel(x, y); if (qRed(px) > 170 && qGreen(px) < 90 && qBlue(px) < 90) red++; }
                if (red > 40) { std::printf("    FLAGS SPLIT (%d red pixels): %s\n", red, qPrintable(c.name)); splitFlags++; }
            }
        }
        std::printf("    %lld kinds of message, %d wider than the window, %d with a split flag\n", (long long)cases.size(), wideCases, splitFlags);
        if (wideCases || splitFlags) failures++;
    });

    /* Consecutive messages from one sender are drawn without a header. Qt's HTML import gave the anchor that finds such a message to the first UTF-16 unit of its text, on its
       own, so an emoji at the start was cut in two (two replacement characters), and in a run of flags every flag after it paired up one off: wrong flags, and no place to break the
       line, so the view grew a horizontal scrollbar. Nothing else here puts messages of one sender next to each other, so this is the only step that draws a message without a header. */
    step("timeline: a message grouped under the previous one is not cut in two by its anchor", 3000, [&] {
        TimelineView g;
        g.resize(900, 600);
        { /* the font as the app sets it */
            QFont f = QApplication::font();
            const QStringList have = QFontDatabase::families();
            for (const char *want : {"Noto Color Emoji", "Twemoji Mozilla", "Twitter Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"}) if (have.contains(want)) { f.setFamilies({QFontInfo(f).family(), want}); break; }
            g.setFont(f);
        }
        g.show();
        QTextBrowser *view = g.findChild<QTextBrowser *>();
        auto *bar = view->horizontalScrollBar();
        /* a text fragment that starts or ends inside a grapheme cluster (a surrogate pair, a flag, a joined sequence, a letter and its accent) is shaped in pieces */
        auto cuts = [](QTextDocument *d) {
            int n = 0;
            for (QTextBlock b = d->begin(); b.isValid(); b = b.next()) {
                const QString text = b.text();
                if (text.isEmpty()) continue;
                QTextBoundaryFinder finder(QTextBoundaryFinder::Grapheme, text);
                for (QTextBlock::Iterator it = b.begin(); !it.atEnd(); ++it) {
                    const int at = it.fragment().position() - b.position();
                    if (at <= 0 || at >= text.size()) continue;
                    finder.setPosition(at);
                    if (!finder.isAtBoundary()) n++;
                }
            }
            return n;
        };
        auto u = [](const char *utf8) { return QString::fromUtf8(utf8); };
        const QString il = u("\xF0\x9F\x87\xAE\xF0\x9F\x87\xB1");
        auto run = [](int n, const QString &unit) { QString t; while (t.size() < n) t += unit; return t.left(n); };
        struct Case { QString name; QJsonObject row; bool flags = false; };
        QList<Case> cases;
        auto text = [&](const QString &n, const QString &t, bool flags = false) { cases.append({n, message(5, t), flags}); }; /* message(5, ...) has the sender of message(0, ...) */
        text("15 Israeli flags", run(60, il), true); text("497 Israeli flags", run(497 * 4, il), true); text("one flag", il, true);
        text("grinning face", u("\xF0\x9F\x98\x80 hello")); text("thumbs up with a skin tone", u("\xF0\x9F\x91\x8D\xF0\x9F\x8F\xBD ok"));
        text("family (joined)", u("\xF0\x9F\x91\xA8\xE2\x80\x8D\xF0\x9F\x91\xA9\xE2\x80\x8D\xF0\x9F\x91\xA7 family")); text("rainbow flag", u("\xF0\x9F\x8F\xB3\xEF\xB8\x8F\xE2\x80\x8D\xF0\x9F\x8C\x88 pride"));
        text("red heart (a character and a variation selector)", u("\xE2\x9D\xA4\xEF\xB8\x8F love")); text("e and a combining accent", u("e\xCC\x81 cafe")); text("keycap", u("1\xEF\xB8\x8F\xE2\x83\xA3 one"));
        cases.append({"formatted text", message(5, "x", u("\xF0\x9F\x98\x80 <b>bold</b>")), false});
        /* Matrix lets a sender put name="..." on a link; that anchor would be given to the first unit of the text after it just the same */
        cases.append({"sender's own <a name> in front of flags", message(5, "x", "<a name=\"x\"></a>" + run(60, il)), true});
        cases.append({"sender's own <a name> on a link that starts with an emoji", message(5, "x", "<a name=\"x\" href=\"https://example.org/\">" + u("\xF0\x9F\x98\x80") + " link</a>"), false});
        { QJsonObject r = message(5, "x"); r["kind"] = "poll"; r["poll"] = QJsonObject{{"question", u("\xF0\x9F\x8D\x95 pizza?")}, {"answers", QJsonArray{QJsonObject{{"id", "a"}, {"text", u("\xF0\x9F\x87\xAE\xF0\x9F\x87\xB1 yes")}, {"votes", 0}, {"mine", false}}}}, {"max_selections", 1}, {"total_votes", 0}, {"ended", false}};
          cases.append({"poll question", r, false}); }
        int cutCases = 0, wideCases = 0, splitFlags = 0;
        for (const Case &c : cases) {
            g.reset();
            g.setRows(QJsonArray{message(0, "before"), c.row});
            QElapsedTimer pause; pause.start(); while (pause.elapsed() < 170) QApplication::processEvents(QEventLoop::AllEvents, 10); /* the view merges redraws within 150 ms */
            excludedMs += pause.elapsed();
            const int n = cuts(view->document());
            if (n) { std::printf("    CUT IN %d PLACES: %s\n", n, qPrintable(c.name)); cutCases++; }
            if (bar->maximum() > 0) { std::printf("    WIDER THAN THE WINDOW by %d px: %s\n", bar->maximum(), qPrintable(c.name)); wideCases++; }
            if (c.flags) { /* red is the neighbours' flags; the Israeli one has none (only the sender's name is dark red, at the top) */
                const QImage img = g.grab().toImage();
                int red = 0;
                for (int y = 100; y < img.height() - 100; y++) for (int x = 40; x < img.width() - 20; x++) { const QRgb px = img.pixel(x, y); if (qRed(px) > 170 && qGreen(px) < 90 && qBlue(px) < 90) red++; }
                if (red > 40) { std::printf("    FLAGS SPLIT (%d red pixels): %s\n", red, qPrintable(c.name)); splitFlags++; }
            }
        }
        { /* the time of a bookmarked message starts with a star and a variation selector, a pinned one with a word, a message with a header has its anchor in front of the time */
            for (int how = 0; how < 2; how++) {
                g.reset();
                if (how == 0) g.setBookmarks({"$e0"}); else g.setPinned({"$e0"});
                g.setRows(QJsonArray{message(0, "hi")});
                QElapsedTimer pause; pause.start(); while (pause.elapsed() < 170) QApplication::processEvents(QEventLoop::AllEvents, 10);
                excludedMs += pause.elapsed();
                const bool shownMarked = view->toPlainText().contains(how == 0 ? QString::fromUtf8("\xE2\x98\x85") : QString("pinned"));
                if (cuts(view->document()) || !shownMarked) { std::printf("    %s: %s (cut in %d places)\n", how == 0 ? "bookmarked message" : "pinned message", shownMarked ? "CUT" : "NOT SHOWN", cuts(view->document())); cutCases++; }
            }
            g.setBookmarks({}); g.setPinned({});
        }
        { /* a message that starts a conversation shows its sender's name: that does not get cut either */
            for (const QString &name : {u("\xF0\x9F\x87\xAE\xF0\x9F\x87\xB1 Dan"), u("\xF0\x9F\x98\x80 Zed")}) {
                QJsonObject r = message(0, "hi"); r["sender"] = name;
                g.reset(); g.setRows(QJsonArray{r});
                QElapsedTimer pause; pause.start(); while (pause.elapsed() < 170) QApplication::processEvents(QEventLoop::AllEvents, 10);
                excludedMs += pause.elapsed();
                if (cuts(view->document())) { std::printf("    CUT: the name %s\n", qPrintable(name)); cutCases++; }
            }
        }
        std::printf("    %lld kinds of grouped message, 2 names, 2 marked times: %d cut, %d wider than the window, %d with a split flag\n", (long long)cases.size(), cutCases, wideCases, splitFlags);
        if (cutCases || wideCases || splitFlags) failures++;
        { /* the anchor of a grouped message still finds it: revealing one in the middle of a long run from one sender puts it at the top of the view */
            QJsonArray many;
            for (int k = 0; k < 150; k++) many.append(message(k * 5, QString("line %1 of a long run from one sender").arg(k)));
            g.reset(); g.setRows(many); settle();
            g.revealMessage("$e500"); settle();
            const QString top = g.eventAt(QPoint(40, 8));
            std::printf("    revealing $e500 in a run of 150 grouped messages: the message at the top is %s\n", qPrintable(top.isEmpty() ? QString("(none)") : top));
            if (top != "$e500") failures++;
        }
    });

    Sidebar sb;
    sb.resize(260, 700);
    sb.show();
    step("sidebar: 5 000 rooms in 40 spaces", 800, [&] {
        QJsonArray rooms;
        for (int i = 0; i < 5000; i++) rooms.append(QJsonObject{{"id", QString("!r%1:hs").arg(i)}, {"title", QString("room number %1").arg(i)}, {"section", QString("space %1").arg(i / 125)}, {"unread", i % 13}, {"highlight", i % 97 == 0}, {"notify", "default"}});
        sb.refresh(rooms, "!r10:hs", "");
    });
    step("sidebar: 200 refreshes while one room's unread count ticks", 2500, [&] {
        QJsonArray rooms;
        for (int i = 0; i < 1000; i++) rooms.append(QJsonObject{{"id", QString("!r%1:hs").arg(i)}, {"title", QString("room %1").arg(i)}, {"section", "Rooms"}, {"unread", 0}, {"notify", "default"}});
        for (int n = 0; n < 200; n++) { QJsonObject r = rooms[3].toObject(); r["unread"] = n; rooms[3] = r; sb.refresh(rooms, "!r1:hs", ""); app.processEvents(); }
    });

    MemberList ml;
    ml.resize(240, 600);
    ml.show();
    step("members: 20 000 people in a room", 1500, [&] {
        QJsonArray ms;
        for (int i = 0; i < 20000; i++) ms.append(QJsonObject{{"user_id", QString("@u%1:hs").arg(i)}, {"name", QString("User %1").arg(i)}, {"role", i < 5 ? "Admin" : i < 50 ? "Moderator" : "Member"}, {"verified", i % 11 == 0}, {"can_kick", true}});
        ml.refresh(QJsonObject{{"id", "!big:hs"}, {"members", ms}});
    });
    step("members: 20 heartbeat flips at a time, 100 times, in 20 000", 3000, [&] {
        QJsonObject st;
        for (int i = 0; i < 20000; i++) st.insert(QString("@u%1:hs").arg(i), "online");
        ml.setPresence(st);
        for (int round = 0; round < 100; round++) {
            for (int k = 0; k < 20; k++) st.insert(QString("@u%1:hs").arg(round * 20 + k), round % 2 ? "online" : "offline");
            ml.setPresence(st);
        }
    });
    step("members: presence flips for 20 000 people 5 times", 5000, [&] {
        for (int round = 0; round < 5; round++) {
            QJsonObject st;
            for (int i = 0; i < 20000; i++) st.insert(QString("@u%1:hs").arg(i), (i + round) % 3 ? "online" : "offline");
            ml.setPresence(st);
        }
    });
    /* a big room's list holds only the people the sync brought until the rest is loaded: it says so, and the note goes when the list is complete */
    step("members: a partial list of a big room says how many people are in it", 300, [&] {
        MemberList ml; /* a list of its own: the other one still holds 20 000 rows to clear */
        ml.resize(240, 600);
        ml.show();
        QJsonArray ms;
        for (int i = 0; i < 30; i++) ms.append(QJsonObject{{"user_id", QString("@p%1:hs").arg(i)}, {"name", QString("Person %1").arg(i)}, {"role", "Member"}});
        ml.refresh(QJsonObject{{"id", "!huge:hs"}, {"members", ms}, {"members_complete", false}, {"member_count", 5000}});
        const QString note = ml.count() ? ml.item(0)->text() : QString();
        const bool partial = note.contains("30 of 5000 people so far");
        ml.refresh(QJsonObject{{"id", "!huge:hs"}, {"members", ms}, {"members_complete", true}, {"member_count", 30}});
        const bool gone = ml.count() > 0 && !ml.item(0)->text().contains("so far");
        std::printf("    note: \"%s\", gone once complete: %s\n", qPrintable(note), gone ? "yes" : "NO");
        if (!partial || !gone) failures++;
    });

    /* the pictures that are sent: their size and, when they are big, a thumbnail for the receivers (the engine has no image decoders) */
    step("pictures: the thumbnails of pictures that are sent", 800, [&] {
        QTemporaryDir dir;
        QElapsedTimer whole, inThumbnails; /* making the test pictures is not what is measured */
        whole.start();
        qint64 spent = 0;
        auto pictureInfo = [&](const QString &path, const QString &d, const QString &name) { inThumbnails.start(); const vc::PictureInfo i = vc::pictureInfo(path, d, name); spent += inThumbnails.nsecsElapsed(); return i; };
        auto save = [&](const QString &name, const QImage &img, const char *fmt, int q = -1) { const QString p = dir.filePath(name); img.save(p, fmt, q); return p; };
        auto check = [&](const char *what, bool ok) { if (!ok) { std::printf("    WRONG: %s\n", what); failures++; } };
        const QString big = save("big.jpg", photo(4000, 3000, 1), "JPEG", 85);
        const PictureInfo b = pictureInfo(big, dir.path(), "t-big");
        const QImage bt(b.thumbPath);
        std::printf("    4000x3000 JPEG of %lld KB: %dx%d, thumbnail %dx%d %s of %lld KB\n", (long long)QFileInfo(big).size() / 1024, b.width, b.height, b.thumbWidth, b.thumbHeight, qPrintable(b.thumbMime), (long long)QFileInfo(b.thumbPath).size() / 1024);
        check("a big JPEG gets its size and a JPEG thumbnail that fits 960x640", b.width == 4000 && b.height == 3000 && b.thumbMime == "image/jpeg" && b.thumbWidth == 853 && b.thumbHeight == 640 && bt.size() == QSize(853, 640) && QFileInfo(b.thumbPath).size() < QFileInfo(big).size() / 5);
        const QString shot = save("shot.png", photo(2560, 1440, 2), "PNG"); /* opaque: JPEG */
        const PictureInfo sh = pictureInfo(shot, dir.path(), "t-shot");
        check("an opaque PNG gets a JPEG thumbnail of the right proportions", sh.width == 2560 && sh.height == 1440 && sh.thumbMime == "image/jpeg" && sh.thumbWidth == 960 && sh.thumbHeight == 540);
        QImage argb = photo(1600, 1200, 3).convertToFormat(QImage::Format_ARGB32); /* an alpha channel that is not used (the clipboard gives these): still JPEG */
        const PictureInfo ar = pictureInfo(save("clip.png", argb, "PNG"), dir.path(), "t-clip");
        check("an unused alpha channel does not make the thumbnail a PNG", ar.thumbMime == "image/jpeg");
        for (int y = 0; y < argb.height(); y++) for (int x = 0; x < argb.width(); x++) argb.setPixel(x, y, qRgba(qRed(argb.pixel(x, y)), 100, 50, y * 255 / argb.height()));
        const PictureInfo tr = pictureInfo(save("alpha.png", argb, "PNG"), dir.path(), "t-alpha");
        const QImage trt(tr.thumbPath);
        check("a picture with transparency gets a PNG thumbnail that keeps it", tr.thumbMime == "image/png" && trt.hasAlphaChannel() && qAlpha(trt.pixel(10, 5)) < 20 && qAlpha(trt.pixel(10, trt.height() - 5)) > 235);
        const PictureInfo sm = pictureInfo(save("small.jpg", photo(300, 200, 4), "JPEG", 85), dir.path(), "t-small");
        check("a small picture has its size and no thumbnail", sm.width == 300 && sm.height == 200 && sm.thumbPath.isEmpty());
        const PictureInfo lowres = pictureInfo(save("noisy.png", photo(700, 500, 5), "PNG"), dir.path(), "t-noisy"); /* big in bytes, small in pixels: nothing to scale to */
        check("a picture that fits 960x640 has no thumbnail whatever its bytes", lowres.width == 700 && lowres.thumbPath.isEmpty());
        /* the EXIF orientation: a picture that is stored sideways is as big as it is shown, and so is its thumbnail (Qt cannot write the orientation: the tag is put into the file by hand) */
        { QFile f(big); f.open(QIODevice::ReadOnly); QByteArray jpeg = f.readAll();
          static const char exif[] = "\xff\xe1\x00\x22" "Exif\0\0" "MM\0\x2a\0\0\0\x08" "\0\x01" "\x01\x12\0\x03\0\0\0\x01\0\x06\0\0" "\0\0\0\0"; /* orientation 6: turned a quarter */
          jpeg.insert(2, QByteArray(exif, 36));
          QFile o(dir.filePath("turned.jpg")); o.open(QIODevice::WriteOnly); o.write(jpeg); o.close();
          const PictureInfo t = pictureInfo(dir.filePath("turned.jpg"), dir.path(), "t-turned");
          const QImage tt(t.thumbPath);
          std::printf("    stored 4000x3000, turned a quarter: %dx%d, thumbnail %dx%d\n", t.width, t.height, t.thumbWidth, t.thumbHeight);
          check("a picture stored sideways is measured as it is shown", t.width == 3000 && t.height == 4000 && t.thumbHeight == 640 && t.thumbWidth == 480 && tt.size() == QSize(480, 640)); }
        /* what the engine does not send as a picture, and what cannot be read */
        QFile bad(dir.filePath("garbage.jpg")); bad.open(QIODevice::WriteOnly); bad.write(QByteArray(400000, 'x')); bad.close();
        check("a file that is no picture gets nothing", pictureInfo(dir.filePath("garbage.jpg"), dir.path(), "t-bad").width == 0 && pictureInfo(dir.filePath("nothing.jpg"), dir.path(), "t-none").width == 0);
        check("a bitmap is sent as a file, with no picture info", pictureInfo(save("a.bmp", photo(1600, 1200, 7), "BMP"), dir.path(), "t-bmp").width == 0);
        excludedMs = whole.nsecsElapsed() / 1e6 - spent / 1e6;
    });

    /* ---- memory: what a long session shows (pictures, avatars) must not pile up. Photos and faces are made here, as real JPEG/PNG files. These steps come
       last: they leave the heap in a state that makes the time steps above slower by a few per cent. Their figures and budgets are for `qvector_stress memory` run
       on its own (a fresh process): in a full run they start from a warm heap that reuses freed memory and read lower. ---- */
    QTemporaryDir farm;
    QStringList photos, faces;
    const bool wantMemory = farm.isValid() && (only.isEmpty() || only.contains("memory", Qt::CaseInsensitive));
    if (wantMemory) {
        for (int i = 0; i < 8; i++) { const QString p = farm.filePath(QString("photo%1.jpg").arg(i)); photo(4000, 3000, 100 + i).save(p, "JPEG", 85); photos << p; }
        { const QString p = farm.filePath("shot.png"); photo(2560, 1440, 7).save(p, "PNG"); photos << p; }
        for (int i = 0; i < 40; i++) { const QString p = farm.filePath(QString("face%1.jpg").arg(i)); photo(512, 512, 500 + i).save(p, "JPEG", 90); faces << p; }
    }
    /* the same file under a name of its own: the views key their pictures by path, so every link counts as another picture */
    auto link = [&](const QString &base, const QString &tag) { const QString p = farm.filePath(tag + "." + QFileInfo(base).suffix()); QFile::link(base, p); return p; };
    /* a room as the engine sends it: 60 messages, 12 of them pictures (some a PNG screenshot), a sticker, a gallery of four, every sender with a face */
    auto room = [&](int r) {
        QJsonArray rows;
        for (int i = 0; i < 60; i++) {
            QJsonObject m = message(i, QString("message %1 of room %2").arg(i).arg(r));
            m["avatar_path"] = link(faces[(r * 5 + i % 5) % faces.size()], QString("face_r%1_s%2").arg(r).arg(i % 5));
            if (i % 5 == 4) { m["kind"] = "image"; m["body"] = "photo.jpg"; m["file_name"] = "photo.jpg"; m["image_w"] = 4000; m["image_h"] = 3000; m["image_path"] = link(photos[(r + i) % photos.size()], QString("pic_r%1_%2").arg(r).arg(i)); }
            else if (i == 7) { m["kind"] = "sticker"; m["image_path"] = link(photos[(r + 3) % photos.size()], QString("stk_r%1").arg(r)); }
            else if (i == 21) {
                m["kind"] = "gallery"; m["body"] = "";
                QJsonArray g;
                for (int k = 0; k < 4; k++) g.append(QJsonObject{{"index", k}, {"kind", "image"}, {"name", "g.jpg"}, {"size", 1000000}, {"image_path", link(photos[(r + k) % photos.size()], QString("gal_r%1_%2").arg(r).arg(k))}});
                m["gallery"] = g;
            }
            rows.append(m);
        }
        return rows;
    };
    if (wantMemory) {
        TimelineView mtl;
        mtl.resize(900, 600);
        mtl.show();
        app.processEvents();
        memstep("memory: 10 rooms of 14 pictures, visited 3 times", 40000, [&] {
            for (int cycle = 0; cycle < 3; cycle++) for (int r = 0; r < 10; r++) { mtl.reset(); mtl.setRows(room(r)); settle(); }
        }, 8000);
        memstep("memory: thumbnails of nine photos that are sent (eight of 12 megapixels)", 8000, [&] {
            int n = 0;
            for (const QString &p : photos) { const PictureInfo i = pictureInfo(p, farm.path(), QString("sent-%1").arg(n++)); if (i.thumbPath.isEmpty()) failures++; }
        });
        mtl.reset();
        Sidebar msb;
        msb.resize(260, 700);
        msb.show();
        memstep("memory: 600 rooms with a picture of their own (sidebar)", 12000, [&] {
            QJsonArray rooms;
            for (int i = 0; i < 600; i++) rooms.append(QJsonObject{{"id", QString("!m%1:hs").arg(i)}, {"title", QString("room %1").arg(i)}, {"section", "Rooms"}, {"unread", 0}, {"notify", "default"}, {"avatar_path", link(faces[i % faces.size()], QString("room_face%1").arg(i))}});
            msb.refresh(rooms, "", "");
        });
        memstep("memory: a room of 100 messages, as the engine sends them", 1500, [&] { mtl.reset(); mtl.setRows(engineRows(100)); settle(); });
        memstep("memory: a room of 5 000 messages loaded, 100 shown", 12000, [&] { mtl.reset(); mtl.setRows(engineRows(5000)); settle(); });
        memstep("memory: the same room scrolled back to a message 3 000 back (a window of 180)", 5000, [&] { mtl.revealMessage("$event2000abcdefghijklmnopqrstuvwxyz0123456789:example.org"); settle(); });
        memstep("memory: the same room read 30 pages upwards (a window of 300)", 8000, [&] {
            mtl.reset(); mtl.setRows(engineRows(5000)); settle();
            auto *br = mtl.findChild<QTextBrowser *>();
            for (int i = 0; i < 30; i++) {
                QWheelEvent w(QPointF(50, 50), QPointF(50, 50), QPoint(), QPoint(0, 120), Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
                QApplication::sendEvent(br->viewport(), &w);
                br->verticalScrollBar()->setValue(0);
                QApplication::processEvents();
            }
            settle();
            std::printf("    window [%d, %d)\n", mtl.shownFirst(), mtl.shownEnd());
        });
        MemberList mml;
        mml.resize(240, 600);
        mml.show();
        memstep("memory: 3 000 people with a picture each (member list)", 60000, [&] {
            QJsonArray ms;
            for (int i = 0; i < 3000; i++) ms.append(QJsonObject{{"user_id", QString("@p%1:hs").arg(i)}, {"name", QString("Person %1").arg(i)}, {"role", "Member"}, {"avatar_path", link(faces[i % faces.size()], QString("member_face%1").arg(i))}});
            mml.refresh(QJsonObject{{"id", "!big:hs"}, {"members", ms}});
        });
    }

    /* The sticker picker is a popup nobody looks at most of the time: it must not decode and scale every sticker of a room's packs whenever the room
       sends them (every room entered did), only when it opens. */
    step("pickers: stickers are drawn when the picker opens", 1500, [&] {
        QTemporaryDir dir;
        const QString png = dir.filePath("sticker.png");
        QImage(300, 300, QImage::Format_RGB32).save(png);
        PackPicker pp;
        QJsonArray emotes;
        for (int i = 0; i < 30; i++) emotes.append(QJsonObject{{"shortcode", QString("s%1").arg(i)}, {"mxc", QString("mxc://hs/s%1").arg(i)}, {"path", png}, {"sticker", true}, {"emoji", false}});
        pp.setPacks(QJsonArray{QJsonObject{{"name", "pack"}, {"emotes", emotes}}});
        auto *grid = pp.findChild<QListWidget *>();
        const int before = grid->count();
        pp.popupAt(QPoint(400, 400));
        const int after = grid->count();
        pp.hide();
        std::printf("    stickers in the grid before opening: %d, after: %d\n", before, after);
        if (before != 0 || after != 30) failures++;
    });

    /* The other side's pictures arrive on a decoder thread and used to be queued to the GUI thread one by one: a window that stalls for a few seconds
       (a big room being laid out, a file dialog, a loaded machine) collected 15 pictures a second of 2 MB each, per person. Only the newest picture of
       a person is worth showing. */
    step("video: 100 pictures of 960x540 while the GUI thread is busy", 3000, [&] {
        QTemporaryDir dir;
        Core core(dir.path());
        int delivered = 0;
        QObject::connect(&core, &Core::remoteFrame, &core, [&](const QString &, const QImage &) { delivered++; });
        const long base = heldKiB();
        std::thread engine([&] { std::vector<unsigned char> px(960 * 540 * 4, 7); for (int i = 0; i < 100; i++) Core::pictureTrampoline(&core, "@bob:hs", 960, 540, px.data()); });
        engine.join(); /* no event has been processed meanwhile: the GUI thread was busy */
        const long waiting = rssAnonKiB() - base;
        QApplication::processEvents();
        std::printf("    while they wait: %ld KiB resident more than before (100 queued pictures are 200 000); delivered afterwards: %d\n", waiting, delivered);
        if (delivered != 1 || waiting > 20000) failures++;
    });

    /* a closed member list holds none of its people */
    memstep("memory: a member list that has been hidden holds no people", 16000, [&] {
        MemberList hidden;
        hidden.resize(240, 600);
        hidden.show();
        QJsonArray ms;
        for (int i = 0; i < 3000; i++) ms.append(QJsonObject{{"user_id", QString("@h%1:hs").arg(i)}, {"name", QString("Hidden person %1").arg(i)}, {"role", "Member"}});
        hidden.refresh(QJsonObject{{"id", "!hide:hs"}, {"members", ms}});
        settle();
        hidden.hide();
        QJsonObject states;
        states.insert("@h1:hs", "online");
        hidden.setPresence(states); /* news about presence must not rebuild a list nobody sees */
        if (hidden.count() != 0) { std::printf("    %d rows are still there\n", hidden.count()); failures++; }
        hidden.show(); /* it fills again by itself (the window coming back from the tray) ... */
        QApplication::processEvents();
        if (hidden.count() != 3000) { std::printf("    %d rows after showing it again, not 3000\n", hidden.count()); failures++; }
        hidden.hide();
        hidden.show(); /* ... but the toggle shows it and fills it with the open room's details at once: the old room's people are not built first */
        hidden.refresh(QJsonObject{{"id", "!other:hs"}, {"members", QJsonArray{QJsonObject{{"user_id", "@z:hs"}, {"name", "Zed"}, {"role", "Member"}}}}});
        QApplication::processEvents();
        if (hidden.count() != 1) { std::printf("    %d rows after a toggle, not the other room's 1\n", hidden.count()); failures++; }
        hidden.hide();
    });

    std::printf("\n%d step(s) too slow\n", failures);
    return failures ? 1 : 0;
}
