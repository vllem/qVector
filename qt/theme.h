#ifndef VC_QT_THEME_H
#define VC_QT_THEME_H

#include <QApplication>
#include <QColor>
#include <QDateTime>
#include <QFont>
#include <QIcon>
#include <QPalette>
#include <QPixmap>
#include <QString>

namespace vc {

enum class Theme { System, Dark, Scheme };

/* Applies the chosen theme: System keeps the desktop's own style and palette, Dark is a Ripcord-like dark Fusion palette. */
void applyTheme(QApplication &app, Theme t);
Theme savedTheme();
void saveTheme(Theme t);
/* A KDE colour scheme file (*.colors) turned into a palette; false if the file has no usable colours. Theme::Scheme applies the saved one. */
bool loadColorScheme(const QString &path, QPalette &out);
QString savedSchemePath();
void saveSchemePath(const QString &path);

/* a colour for a user / room name that reads well on the current palette (light or dark) */
QColor nameColor(const QString &seed, const QPalette &pal);
bool isDark(const QPalette &pal);

/* the colour emoji font the app found or loaded (empty if none): used wherever emoji are drawn */
QString emojiFamily();
void setEmojiFamily(const QString &family);

/* How dates and times of messages are written (Preferences > Timestamps). Qt format strings, e.g. "dd/MM/yyyy" and "HH:mm". */
QString dateFormat();
QString timeFormat();
void setTimestampFormats(const QString &date, const QString &time);
/* a message time: just the time for today, date and time otherwise (always both with allDates); withSeconds adds :ss to the time */
QString formatWhen(const QDateTime &dt, bool allDates = false, bool withSeconds = false);

/* the application icon, and the same with a count of unread messages for the tray */
QIcon appIcon();
QPixmap trayPixmap(int unread, int size = 64);
/* a short two-note chime as a WAV file in the cache directory (written once); empty on failure */
QString notificationSoundFile();

}

#endif
