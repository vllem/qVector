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
#include <QStandardPaths>
#include <QTimer>
#include <cstring>

/* Emoji (reactions, messages) need a colour emoji font. Use the system one when there is one; otherwise load the Noto Color Emoji shipped
   in third_party/fonts, found next to the build tree or in an install prefix. */
/* Does the default font draw a colour emoji? Draws U+1F600 and looks for coloured pixels: a missing glyph (a box) and a monochrome one have none. */
static bool emojiRendersInColour()
{
    QImage img(64, 64, QImage::Format_RGB32);
    img.fill(Qt::white);
    QPainter p(&img);
    QFont f = QApplication::font();
    f.setPixelSize(40);
    p.setFont(f);
    p.setPen(Qt::black);
    p.drawText(QRect(0, 0, 64, 64), Qt::AlignCenter, QString::fromUtf8("\xF0\x9F\x98\x80"));
    p.end();
    for (int y = 0; y < img.height(); y++)
        for (int x = 0; x < img.width(); x++) {
            const QColor c = img.pixelColor(x, y);
            if (c.saturation() > 90 && c.value() > 60) return true;
        }
    return false;
}

/* Colour emoji need an emoji font Qt can fall back to. If the default font cannot draw one, load the Noto Color Emoji shipped in
   third_party/fonts (Qt registers application fonts with fontconfig, so its normal fallback then finds it) and check again. The picker
   widgets that show only emoji ask for the family by name (vc::emojiFamily); nothing else changes the application font: symbols such as
   the arrows of the toolbar would otherwise turn into coloured emoji. */
static void ensureEmojiFont()
{
    QString family;
    const QStringList have = QFontDatabase::families();
    for (const char *want : {"Noto Color Emoji", "Twemoji Mozilla", "Twitter Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"})
        if (have.contains(want)) { family = want; break; }
    const bool systemWorks = emojiRendersInColour();
    fprintf(stderr, "font: emoji font of the system: %s; colour emoji %s with the default font\n", family.isEmpty() ? "none" : family.toUtf8().constData(), systemWorks ? "draw" : "do NOT draw");
    if (!systemWorks || family.isEmpty()) {
        const QString exe = QCoreApplication::applicationDirPath();
        const QStringList dirs = {exe + "/fonts", exe + "/../Resources/fonts" /* macOS bundle */, exe + "/../third_party/fonts", exe + "/../../third_party/fonts", exe + "/third_party/fonts", exe + "/../share/vector/fonts",
                                  "/usr/local/share/vector/fonts", "/usr/share/vector/fonts"};
        for (const QString &d : dirs) {
            const QString f = QDir(d).filePath("NotoColorEmoji-Regular.ttf");
            if (!QFileInfo::exists(f)) continue;
            const int id = QFontDatabase::addApplicationFont(f);
            const QStringList fams = id >= 0 ? QFontDatabase::applicationFontFamilies(id) : QStringList();
            if (fams.isEmpty()) { fprintf(stderr, "font: %s could not be loaded\n", f.toUtf8().constData()); continue; }
            family = fams.first();
            fprintf(stderr, "font: loaded %s (%s); colour emoji now %s\n", f.toUtf8().constData(), family.toUtf8().constData(), emojiRendersInColour() ? "draw" : "still do NOT draw");
            break;
        }
    }
    if (family.isEmpty()) { fprintf(stderr, "font: no colour emoji font found (install fonts-noto-color-emoji); emoji will show as boxes\n"); return; }
    vc::setEmojiFamily(family);
    {   /* The normal font first (by its real name: the default "Sans Serif" is only an alias and would be skipped, letting the emoji font
           draw digits and spaces), the emoji font behind it, so emoji codepoints come out in colour everywhere. */
        QFont f = QApplication::font();
        const QString base = QFontInfo(f).family();
        f.setFamilies({base, family});
        QApplication::setFont(f);
        fprintf(stderr, "font: application font is \"%s\" with \"%s\" for emoji; colour emoji %s\n", base.toUtf8().constData(), family.toUtf8().constData(), emojiRendersInColour() ? "draw" : "still do NOT draw");
    }
}


int main(int argc, char **argv)
{
#ifdef VC_HAVE_WEBENGINE
    QCoreApplication::setAttribute(Qt::AA_ShareOpenGLContexts); /* Qt WebEngine asks for this before the application object exists */
#endif
    QApplication app(argc, argv);
    QApplication::setApplicationName("vector");
    QApplication::setOrganizationName("vector");
    QApplication::setApplicationDisplayName("qVector");
    QApplication::setDesktopFileName("qvector");
    bool demo = false, members = false;
    QString shot, room, dataDir, dialog, search, thread, server, user, password;
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
    vc::applyTheme(app, vc::savedTheme());
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
