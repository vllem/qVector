#include "qt/emoji.h"
#include "qt/emoji_data.h"
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSettings>
#include <algorithm>

namespace vc {

const QList<Emoji> &allEmoji()
{
    static const QList<Emoji> list = [] {
        QList<Emoji> l;
        l.reserve(int(sizeof EMOJI_ITEMS / sizeof *EMOJI_ITEMS));
        for (const EmojiItem &it : EMOJI_ITEMS) l.append({QString::fromUtf8(it.glyph), QString::fromUtf8(it.name), it.group});
        return l;
    }();
    return list;
}

QStringList emojiGroups()
{
    QStringList g;
    for (const char *n : EMOJI_GROUPS) g << QString::fromUtf8(n);
    return g;
}

QList<Emoji> searchEmoji(const QString &query, int limit)
{
    const QStringList words = query.toLower().split(' ', Qt::SkipEmptyParts);
    QList<Emoji> exact, rest;
    if (words.isEmpty()) return {};
    for (const Emoji &e : allEmoji()) {
        bool all = true;
        for (const QString &w : words) if (!e.name.contains(w, Qt::CaseInsensitive)) { all = false; break; }
        if (!all) continue;
        (e.name.startsWith(words.first(), Qt::CaseInsensitive) ? exact : rest).append(e);
        if (exact.size() + rest.size() >= limit * 3) break;
    }
    exact += rest;
    return exact.mid(0, limit);
}

QStringList recentEmoji() { return QSettings("vector", "vector").value("recent_emoji").toStringList(); }

void rememberEmoji(const QString &glyph)
{
    QSettings s("vector", "vector");
    QStringList l = s.value("recent_emoji").toStringList();
    l.removeAll(glyph);
    l.prepend(glyph);
    while (l.size() > 32) l.removeLast();
    s.setValue("recent_emoji", l);
}

QList<Sticker> savedStickers()
{
    QList<Sticker> out;
    const QJsonArray arr = QJsonDocument::fromJson(QSettings("vector", "vector").value("stickers").toByteArray()).array();
    for (const QJsonValue &v : arr) {
        const QJsonObject o = v.toObject();
        Sticker s;
        s.mxc = o["mxc"].toString(); s.body = o["body"].toString(); s.mime = o["mime"].toString(); s.w = o["w"].toInt(); s.h = o["h"].toInt();
        if (s.mxc.startsWith("mxc://")) out.append(s);
    }
    return out;
}

static void storeStickers(const QList<Sticker> &list)
{
    QJsonArray arr;
    for (const Sticker &s : list) arr.append(QJsonObject{{"mxc", s.mxc}, {"body", s.body}, {"mime", s.mime}, {"w", s.w}, {"h", s.h}});
    QSettings("vector", "vector").setValue("stickers", QJsonDocument(arr).toJson(QJsonDocument::Compact));
}

void saveSticker(const Sticker &s)
{
    QList<Sticker> l = savedStickers();
    for (const Sticker &x : l) if (x.mxc == s.mxc) return;
    l.append(s);
    storeStickers(l);
}

void removeSticker(const QString &mxc)
{
    QList<Sticker> l = savedStickers();
    l.erase(std::remove_if(l.begin(), l.end(), [&](const Sticker &s) { return s.mxc == mxc; }), l.end());
    storeStickers(l);
}

}
