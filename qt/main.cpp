#include "qt/core.h"
#include "qt/emoji.h"
#include "qt/mainwindow.h"
#include "qt/theme.h"
#include <QApplication>
#include <QCommandLineParser>
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

/* Colour emoji come from the system's emoji font (Noto Color Emoji, Twemoji, Apple Color Emoji, Segoe UI Emoji). Qt's own fallback does not always
   reach it, so it is added behind the normal font by name. The picker widgets that show only emoji ask for the family by name (vc::emojiFamily);
   nothing else changes the application font: symbols such as the arrows of the toolbar would otherwise turn into coloured emoji. */
static void ensureEmojiFont()
{
    QString family;
    const QStringList have = QFontDatabase::families();
    for (const char *want : {"Noto Color Emoji", "Twemoji Mozilla", "Twitter Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"})
        if (have.contains(want)) { family = want; break; }
    if (family.isEmpty()) { qWarning("font: no colour emoji font found (install fonts-noto-color-emoji); emoji will show as boxes"); return; }
    vc::setEmojiFamily(family);
    /* The normal font first (by its real name: the default "Sans Serif" is only an alias and would be skipped, letting the emoji font
       draw digits and spaces), the emoji font behind it, so emoji codepoints come out in colour everywhere. */
    QFont f = QApplication::font();
    const QString base = QFontInfo(f).family();
    f.setFamilies({base, family});
    QApplication::setFont(f);
    qInfo("font: application font is \"%s\" with \"%s\" for emoji", qUtf8Printable(base), qUtf8Printable(family));
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
        QObject::connect(trim, &QTimer::timeout, trim, [] { malloc_trim(0); });
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
    /* Everything but --help is a dev aid. parse(), not process(): an option this build does not know is ignored, as it always was. */
    QCommandLineParser cli;
    cli.setApplicationDescription("qVector, a Matrix client");
    const QCommandLineOption helpOpt = cli.addHelpOption();
    const QCommandLineOption demoOpt("demo", "Run against an in-process fake homeserver (no account, no network)."),
        shotOpt("screenshot", "Save a picture of the main window (or the dialog being shown) and quit.", "png"),
        delayOpt("delay", "Milliseconds before the screenshot; the other dev aids run at half of it.", "ms", "6000"),
        roomOpt("room", "Open the room with this name.", "name"),
        dataOpt("data", "Account data directory (default: $VECTOR_DATA, else the app data location).", "dir"),
        dialogOpt("dialog", "Open a dialog: poll, saved, prefs, verify, recovery, settings, picture, source, ...", "name"),
        membersOpt("members", "Show the member list."),
        serverOpt("server", "Sign in to this homeserver (with --user and --password).", "url"),
        userOpt("user", "User name for --server.", "user"),
        passwordOpt("password", "Password for --server.", "password"),
        sendFileOpt("send-file", "Send these files (comma-separated) into the room that --room opens.", "files"),
        sendTextOpt("send-text", "Send the text of each file (comma-separated, one message each) into the room that --room opens.", "files"),
        verifyOpt("verify", "Start a verification."),
        confirmOpt("confirm", "Confirm a verification by itself."),
        threadOpt("thread", "Open the thread of the message with this text.", "text"),
        searchOpt("search", "Search the room for this text.", "text"),
        iconsOpt("write-icons", "Write the application icon as icon-<size>.png into a directory and quit (packaging).", "dir");
    cli.addOptions({demoOpt, shotOpt, delayOpt, roomOpt, dataOpt, dialogOpt, membersOpt, serverOpt, userOpt, passwordOpt, sendFileOpt, sendTextOpt, verifyOpt,
                    confirmOpt, threadOpt, searchOpt, iconsOpt});
    if (!cli.parse(app.arguments())) qWarning("%s", qUtf8Printable(cli.errorText()));
    if (cli.isSet(helpOpt)) cli.showHelp();
    if (cli.isSet(iconsOpt)) {
        for (int sz : {16, 32, 48, 64, 128, 256, 512}) vc::trayPixmap(0, sz).save(QDir(cli.value(iconsOpt)).filePath(QString("icon-%1.png").arg(sz)));
        return 0;
    }
    const bool demo = cli.isSet(demoOpt), members = cli.isSet(membersOpt), verify = cli.isSet(verifyOpt), confirm = cli.isSet(confirmOpt);
    const QString shot = cli.value(shotOpt), room = cli.value(roomOpt), dialog = cli.value(dialogOpt), search = cli.value(searchOpt), thread = cli.value(threadOpt),
                  server = cli.value(serverOpt), user = cli.value(userOpt), password = cli.value(passwordOpt), sendFile = cli.value(sendFileOpt),
                  sendText = cli.value(sendTextOpt);
    QString dataDir = cli.value(dataOpt);
    const int delay = cli.value(delayOpt).toInt();
    if (qEnvironmentVariableIsSet("VC_DEMO_FONT")) { /* dev aid: the font a desktop theme would give, "Family" or "Family,pixel size" (an offscreen run has only Qt's default) */
        const QStringList fp = qEnvironmentVariable("VC_DEMO_FONT").split(',');
        QFont f(fp[0]);
        if (fp.size() > 1 && fp[1].toInt() > 0) f.setPixelSize(fp[1].toInt());
        QApplication::setFont(f);
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
        if (!sendFile.isEmpty()) QTimer::singleShot(delay / 2 + 4000, &w, [&] { w.sendFilesForDemo(sendFile.split(','), "a gallery"); });
        int sentAt = delay / 2 + 1000;
        for (const QString &path : sendText.split(',', Qt::SkipEmptyParts)) { /* one message per file, in order */
            QTimer::singleShot(sentAt, &w, [&w, path] { QFile f(path); if (f.open(QIODevice::ReadOnly)) w.sendForDemo(QString::fromUtf8(f.readAll())); });
            sentAt += 1200;
        }
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
