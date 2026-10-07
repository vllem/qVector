#ifndef VC_QT_AVATAR_H
#define VC_QT_AVATAR_H

#include <QPixmap>
#include <QString>
#include <QPalette>

namespace vc {

/* Ripcord-style default avatar: a coloured rounded square with an initial. `size` is in device-independent pixels. */
QPixmap avatarPixmap(const QString &seed, const QString &label, int size, qreal dpr, const QPalette &pal);

/* The profile picture in the file `imagePath` (the engine downloads avatars into its cache) as a rounded square, or the letter avatar while it is
   missing / loading / broken. *loaded tells which of the two you got. */
QPixmap profilePixmap(const QString &imagePath, const QString &seed, const QString &label, int size, qreal dpr, const QPalette &pal, bool *loaded = nullptr);

/* the avatar with a small status dot at its corner: online (green), away (orange), offline (grey); unchanged for an unknown state */
QPixmap withPresenceDot(const QPixmap &avatar, const QString &state, const QPalette &pal);

/* the small shield shown next to verified people and messages: kind 0 = verified (green check), 1 = untrusted device (orange !), 2 = changed identity (red !) */
QPixmap shieldPixmap(int kind, int size, qreal dpr);

}

#endif
