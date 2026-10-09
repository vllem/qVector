/* Torture tests for the Qt widgets: `qvector_stress` builds the real TimelineView, Sidebar and MemberList with absurd amounts of data and fails
   when a step is slower than a person would tolerate. Not part of the normal build:

       cmake --build build --target qvector_stress && QT_QPA_PLATFORM=offscreen build/qvector_stress

   Budgets are for a Release build; a Debug/ASan build gets 8x. Pass a name part to run only matching steps. */
#include <QApplication>
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonObject>
#include <QScrollBar>
#include <QTextBrowser>
#include <cstdio>
#include "qt/memberlist.h"
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

    step("timeline: open a room of 50 000 messages", 600, [&] { tl.setRows(messages(50000)); });
    step("timeline: scroll to the top and back 20 times", 800, [&] {
        auto *bar = tl.findChild<QTextBrowser *>()->verticalScrollBar();
        for (int i = 0; i < 20; i++) { bar->setValue(bar->minimum()); app.processEvents(); bar->setValue(bar->maximum()); app.processEvents(); }
    });
    step("timeline: 300 new messages arriving one by one in 50k", 1500, [&] {
        QJsonArray rows = messages(50000);
        qint64 build = 0; /* the harness copy of the array (QJsonArray detaches on append) is not the view's cost */
        for (int i = 0; i < 300; i++) { QElapsedTimer b; b.start(); rows.append(message(50000 + i, "new")); build += b.nsecsElapsed(); tl.setRows(rows); app.processEvents(); }
        excludedMs = build / 1e6;
    });
    step("timeline: 20 000 event-id look-ups (reply, reaction, jump)", 300, [&] {
        tl.setRows(messages(20000));
        for (int i = 0; i < 20000; i += 1) tl.row(QString("$e%1").arg(19999 - i % 40)); /* near the end, like real traffic */
    });
    step("timeline: look up ids at the very start 2000 times", 300, [&] { for (int i = 0; i < 2000; i++) tl.row("$e0"); });
    step("timeline: reveal an old message in 20k rows", 300, [&] { tl.revealMessage("$e5"); });

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
    step("timeline: 100 messages with 500 reactions each", 1500, [&] {
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
    step("timeline: resize the window 100 times with 100 rows", 1200, [&] {
        tl.setRows(messages(100));
        for (int i = 0; i < 100; i++) { tl.resize(500 + (i * 37) % 700, 400 + (i * 53) % 400); app.processEvents(); }
    });
    step("timeline: the same rows set 500 times (must be skipped)", 300, [&] { const QJsonArray rows = messages(100); for (int i = 0; i < 500; i++) tl.setRows(rows); });
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
        for (int i = 0; i < 5000; i++) rooms.append(QJsonObject{{"id", QString("!r%1:hs").arg(i)}, {"title", QString("room number %1").arg(i)}, {"section", QString("space %1").arg(i % 40)}, {"unread", i % 13}, {"highlight", i % 97 == 0}, {"notify", "default"}});
        sb.refresh(rooms, "!r10:hs", "");
    });
    step("sidebar: 200 refreshes while one room's unread count ticks", 1500, [&] {
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
    step("members: presence flips for 20 000 people 5 times", 3000, [&] {
        for (int round = 0; round < 5; round++) {
            QJsonObject st;
            for (int i = 0; i < 20000; i++) st.insert(QString("@u%1:hs").arg(i), (i + round) % 3 ? "online" : "offline");
            ml.setPresence(st);
        }
    });

    std::printf("\n%d step(s) too slow\n", failures);
    return failures ? 1 : 0;
}
