#include "qt/core.h"
#include "qt/emoji.h"
#include "qt/mainwindow.h"
#include "qt/theme.h"
#include <QApplication>
#include <QCoreApplication>
#include <QDialog>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QFontDatabase>
#include <QImage>
#include <QJsonObject>
#include <QPainter>
#include <QPixmapCache>
#ifdef __GLIBC__
#include <malloc.h>
#endif
#include <QStandardPaths>
#include <QStyleHints>
#include <QTimer>
#include <cstring>

/* Colour emoji come from the system's emoji font (Noto Color Emoji, Twemoji, Apple Color Emoji, Segoe UI Emoji). Qt's own fallback does not always
   reach it, so it is added behind the normal font by name. The picker widgets that show only emoji ask for the family by name (vc::emojiFamily);
   nothing else changes the application font: symbols such as the arrows of the toolbar would otherwise turn into coloured emoji. */
static void ensureEmojiFont()
{
    QString family;
    const QStringList have = QFontDatabase::families();
    for (const char *want : {"Noto Color Emoji", "Twemoji Mozilla", "Twitter Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"})
        if (have.contains(want)) { family = want; break; }
    if (family.isEmpty()) { fprintf(stderr, "font: no colour emoji font found (install fonts-noto-color-emoji); emoji will show as boxes\n"); return; }
    vc::setEmojiFamily(family);
    /* The normal font first (by its real name: the default "Sans Serif" is only an alias and would be skipped, letting the emoji font
       draw digits and spaces), the emoji font behind it, so emoji codepoints come out in colour everywhere. */
    QFont f = QApplication::font();
    const QString base = QFontInfo(f).family();
    f.setFamilies({base, family});
    QApplication::setFont(f);
    fprintf(stderr, "font: application font is \"%s\" with \"%s\" for emoji\n", base.toUtf8().constData(), family.toUtf8().constData());
}


int main(int argc, char **argv)
{
#ifdef __GLIBC__
    mallopt(M_ARENA_MAX, 2); /* by default every thread gets an arena that keeps its freed memory */
#endif
    QApplication app(argc, argv);
#ifdef __GLIBC__
    { /* give freed heap pages back to the system now and then */
        auto *trim = new QTimer(&app);
        QObject::connect(trim, &QTimer::timeout, [] { malloc_trim(0); });
        trim->start(30000);
    }
#endif
    QPixmapCache::setCacheLimit(8 * 1024); /* KiB */
    QApplication::setApplicationName("vector");
    QApplication::setOrganizationName("vector");
    QApplication::setApplicationDisplayName("qVector");
    QApplication::setDesktopFileName("qvector");
    if (qEnvironmentVariableIsSet("VC_DEMO_DARK")) { /* dev aid: a dark KDE-like palette without KDE */
        QApplication::setStyle("Fusion");
        QPalette p;
        p.setColor(QPalette::Window, QColor(0x2a, 0x2e, 0x32)); p.setColor(QPalette::WindowText, QColor(0xfc, 0xfc, 0xfc));
        p.setColor(QPalette::Base, QColor(0x1b, 0x1e, 0x20)); p.setColor(QPalette::AlternateBase, QColor(0x23, 0x26, 0x29));
        p.setColor(QPalette::Text, QColor(0xfc, 0xfc, 0xfc)); p.setColor(QPalette::Button, QColor(0x31, 0x36, 0x3b));
        p.setColor(QPalette::ButtonText, QColor(0xfc, 0xfc, 0xfc)); p.setColor(QPalette::Highlight, QColor(0x3d, 0xae, 0xe9));
        p.setColor(QPalette::HighlightedText, QColor(0xfc, 0xfc, 0xfc)); p.setColor(QPalette::PlaceholderText, QColor(0xa1, 0xa9, 0xb1));
        QApplication::setPalette(p);
    }
    bool demo = false, members = false;
    QString shot, room, dataDir, dialog, search, thread, server, user, password, sendFile, sendText;
    bool verify = false, confirm = false;
    int delay = 6000;
    const QStringList args = app.arguments();
    for (int i = 1; i < args.size(); i++) {
        if (args[i] == "--demo") demo = true; /* a fake homeserver, no account and no network */
        else if (args[i] == "--screenshot" && i + 1 < args.size()) shot = args[++i];
        else if (args[i] == "--delay" && i + 1 < args.size()) delay = args[++i].toInt();
        else if (args[i] == "--room" && i + 1 < args.size()) room = args[++i];
        else if (args[i] == "--data" && i + 1 < args.size()) dataDir = args[++i];
        else if (args[i] == "--dialog" && i + 1 < args.size()) dialog = args[++i];
        else if (args[i] == "--members") members = true;
        else if (args[i] == "--server" && i + 1 < args.size()) server = args[++i]; /* dev aid: sign in from the command line (with --user and --password) */
        else if (args[i] == "--user" && i + 1 < args.size()) user = args[++i];
        else if (args[i] == "--password" && i + 1 < args.size()) password = args[++i];
        else if (args[i] == "--send-file" && i + 1 < args.size()) sendFile = args[++i]; /* dev aid: send this file into the room that --room opens */
        else if (args[i] == "--send-text" && i + 1 < args.size()) sendText = args[++i]; /* dev aid: send the text of this file as a message into the room that --room opens */
        else if (args[i] == "--verify") verify = true;
        else if (args[i] == "--confirm") confirm = true;
        else if (args[i] == "--thread" && i + 1 < args.size()) thread = args[++i];
        else if (args[i] == "--search" && i + 1 < args.size()) search = args[++i];
        else if (args[i] == "--write-icons" && i + 1 < args.size()) { /* dev aid for packaging: the application icon as PNG files (icon-<size>.png) into a directory */
            for (int sz : {16, 32, 48, 64, 128, 256, 512}) vc::trayPixmap(0, sz).save(QDir(args[i + 1]).filePath(QString("icon-%1.png").arg(sz)));
            return 0;
        }
    }
    ensureEmojiFont();
    app.setWindowIcon(vc::appIcon());
    app.setQuitOnLastWindowClosed(false); /* the tray icon may keep us running; closeEvent decides */
    if (dataDir.isEmpty()) dataDir = qEnvironmentVariable("VECTOR_DATA", QStandardPaths::writableLocation(QStandardPaths::AppDataLocation) + "/account");
    QDir().mkpath(dataDir);
    QFile::setPermissions(dataDir, QFile::ReadOwner | QFile::WriteOwner | QFile::ExeOwner);
    int rc;
    {
        vc::Core core(dataDir);
        vc::MainWindow w(&core, demo);
        w.show();
        if (demo) {
            const QJsonObject fake = core.call("start_fake_server").toObject();
            core.call("login", {{"homeserver", fake["homeserver"].toString()}, {"user", "alice"}, {"password", "x"}, {"passphrase", ""}});
        } else if (!server.isEmpty()) {
            core.call("login", {{"homeserver", server}, {"user", user}, {"password", password}, {"passphrase", ""}});
        } else w.start();
        w.setAutoConfirm(confirm);
        if (verify) QTimer::singleShot(delay / 2, &w, [&] { w.verifyForDemo(); });
        if (!room.isEmpty()) QTimer::singleShot(delay / 2, &w, [&] { w.openRoomByTitle(room); });
        if (!thread.isEmpty()) QTimer::singleShot(delay / 2 + 500, &w, [&] { w.threadForDemo(thread); });
        if (!sendFile.isEmpty()) QTimer::singleShot(delay / 2 + 4000, &w, [&] { core.call("send_files", {{"paths", QJsonArray::fromStringList(sendFile.split(','))}, {"caption", "a gallery"}}); });
        if (!sendText.isEmpty()) QTimer::singleShot(delay / 2 + 1000, &w, [&] { QFile f(sendText); if (f.open(QIODevice::ReadOnly)) w.sendForDemo(QString::fromUtf8(f.readAll())); });
        if (members) QTimer::singleShot(delay / 2 + 500, &w, [&] { w.membersForDemo(); });
        if (!search.isEmpty()) QTimer::singleShot(delay / 2 + 500, &w, [&] { w.searchForDemo(search); });
        if (!dialog.isEmpty()) QTimer::singleShot(delay / 2 + 500, &w, [&] { w.dialogForDemo(dialog); });
        if (!shot.isEmpty()) QTimer::singleShot(delay, &app, [&] {
            QWidget *target = &w;
            if (!dialog.isEmpty()) for (QWidget *t : QApplication::topLevelWidgets()) if (t != &w && t->isVisible() && qobject_cast<QDialog *>(t)) target = t; /* the dialog being demonstrated */
            target->grab().save(shot);
            app.quit();
        });
        rc = app.exec();
    }
    return rc;
}
