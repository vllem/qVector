/* Torture tests for the Qt widgets: `qvector_stress` builds the real TimelineView, Sidebar and MemberList with absurd amounts of data and fails
   when a step is slower than a person would tolerate. Not part of the normal build:

       cmake --build build --target qvector_stress && QT_QPA_PLATFORM=offscreen build/qvector_stress

   Budgets are for a Release build; a Debug/ASan build gets 8x. Pass a name part to run only matching steps. */
#include <QApplication>
#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
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
#include <QTextBrowser>
#include <QTextDocument>
#include <QWheelEvent>
#include <cstdio>
#ifdef __GLIBC__
#include <malloc.h>
#endif
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
static long heldKiB()
{
    settle();
#ifdef __GLIBC__
    malloc_trim(0);
#endif
    return rssAnonKiB();
}

/* Like step(), for memory: `f` may grow the process by `budgetKiB` at most (resident anonymous memory after everything has settled). The budgets are
   for a screen at scale 1; pictures are drawn at the device's pixels, so at scale 2 (QT_SCALE_FACTOR=2) they have four times the bytes. */
template <class F> static void memstep(const char *name, long budgetKiB, F f)
{
    if (!only.isEmpty() && !QString(name).contains(only, Qt::CaseInsensitive)) return;
    const double scale = qMax(1.0, QGuiApplication::primaryScreen()->devicePixelRatio());
    budgetKiB = long(budgetKiB * scale * scale);
    const long before = heldKiB();
    f();
    const long grew = heldKiB() - before;
    const bool ok = before < 0 || grew <= budgetKiB;
    std::printf("%-58s %+8ld KiB  (budget %ld)  %s\n", name, grew, budgetKiB, ok ? "ok" : "TOO MUCH MEMORY");
    std::fflush(stdout);
    if (!ok) failures++;
}

/* a row with every field the engine sends (core-rs `UiMessage`), as it comes off the wire: JSON text parsed by Qt, which is what the view really holds per message */
static QJsonArray engineRows(int n)
{
    QJsonArray src;
    for (int i = 0; i < n; i++)
        src.append(QJsonObject{{"id", QString("$event%1abcdefghijklmnopqrstuvwxyz0123456789:example.org").arg(i)}, {"sender", "Alice Example"}, {"sender_id", "@alice:example.org"},
                               {"body", QString("message %1: the quick brown fox jumps over the lazy dog, again and again until the line wraps around the window").arg(i)}, {"html", ""}, {"kind", "text"},
                               {"time", "12:34"}, {"ts", 1700000000000.0 + i}, {"own", false}, {"edited", false}, {"pending", false}, {"reply", QJsonValue()}, {"reactions", QJsonArray()},
                               {"image_path", ""}, {"image_w", 0}, {"image_h", 0}, {"file_name", ""}, {"text_file", false}, {"size", 0}, {"seen_by", QJsonArray()}, {"thread_replies", 0},
                               {"preview", QJsonValue()}, {"shield", QJsonValue()}, {"pinned", false}, {"poll", QJsonValue()}, {"gallery", QJsonArray()},
                               {"avatar_path", "/home/user/.local/share/vector/vector/account/media/avatars/mxcexampleorgabcdefghijklmnop.img"}, {"emoji", QJsonArray()}});
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
    QApplication app(argc, argv);
    if (argc > 1) only = argv[1];

    TimelineView tl;
    tl.resize(900, 600);
    tl.show();
    app.processEvents();

    /* ---- memory: what a long session shows (pictures, avatars) must not pile up. Photos and faces are made here, as real JPEG/PNG files. ---- */
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
        memstep("memory: a room of 5 000 messages loaded, 100 shown", 17000, [&] { mtl.reset(); mtl.setRows(engineRows(5000)); settle(); });
        memstep("memory: the same room scrolled back, 3 000 shown", 28000, [&] { mtl.revealMessage("$event2000abcdefghijklmnopqrstuvwxyz0123456789:example.org"); settle(); });
        MemberList mml;
        mml.resize(240, 600);
        mml.show();
        memstep("memory: 3 000 people with a picture each (member list)", 60000, [&] {
            QJsonArray ms;
            for (int i = 0; i < 3000; i++) ms.append(QJsonObject{{"user_id", QString("@p%1:hs").arg(i)}, {"name", QString("Person %1").arg(i)}, {"role", "Member"}, {"avatar_path", link(faces[i % faces.size()], QString("member_face%1").arg(i))}});
            mml.refresh(QJsonObject{{"id", "!big:hs"}, {"members", ms}});
        });
    }

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
    /* Jumping lays out every message from the target to the end of the room (about 1 ms each, see CLAUDE.md), so a jump 1500 rows back is the
       realistic bound; a jump to row 5 of 20 000 freezes the window for a minute (known limit, not asserted). */
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

    std::printf("\n%d step(s) too slow\n", failures);
    return failures ? 1 : 0;
}
