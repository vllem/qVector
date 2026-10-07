#ifndef VC_QT_EMOJI_H
#define VC_QT_EMOJI_H

#include <QList>
#include <QString>
#include <QStringList>

namespace vc {

struct Emoji { QString glyph, name; int group; };

const QList<Emoji> &allEmoji();
QStringList emojiGroups();
/* emoji whose name contains every word of the query, names that start with the first word first; at most `limit` */
QList<Emoji> searchEmoji(const QString &query, int limit = 60);
/* most recently used first; persisted */
QStringList recentEmoji();
void rememberEmoji(const QString &glyph);

/* Pictures the user saved to send as stickers (mxc:// uris of already uploaded media), persisted. */
struct Sticker { QString mxc, body, mime; int w = 0, h = 0; };
QList<Sticker> savedStickers();
void saveSticker(const Sticker &s);
void removeSticker(const QString &mxc);

}

#endif
