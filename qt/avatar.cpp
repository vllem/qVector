#include "qt/avatar.h"
#include "qt/imageload.h"
#include "qt/theme.h"
#include <QCache>
#include <QImage>
#include <QFont>
#include <QPainter>
#include <QPainterPath>

namespace vc {

QPixmap avatarPixmap(const QString &seed, const QString &label, int size, qreal dpr, const QPalette &pal)
{
    QString text = label.isEmpty() ? seed : label;
    while (!text.isEmpty() && (text[0] == '@' || text[0] == '#' || text[0] == '!')) text.remove(0, 1);
    QString initial = text.isEmpty() ? QStringLiteral("?") : QString(text.left(text.at(0).isHighSurrogate() ? 2 : 1)).toUpper();
    QColor bg = nameColor(seed, pal);
    bg.setHsl(bg.hslHue(), 140, isDark(pal) ? 150 : 135);
    /* The picture depends only on the initial, the colour and the size; a list redraws the same few hundred of them again and again. The colour comes
       from a hash of the name (360 hues), so over a long session the combinations are many: the cache is bounded (by bytes) instead of growing. */
    static QCache<QString, QPixmap> cache(4 << 20);
    const QString key = initial + QChar(1) + QString::number(bg.rgb()) + QChar(1) + QString::number(size) + QChar(1) + QString::number(dpr);
    if (const QPixmap *hit = cache.object(key)) return *hit;
    QPixmap pm(QSize(size, size) * dpr);
    pm.setDevicePixelRatio(dpr);
    pm.fill(Qt::transparent);
    QPainter p(&pm);
    p.setRenderHint(QPainter::Antialiasing);
    QPainterPath path;
    path.addRoundedRect(QRectF(0, 0, size, size), size * 0.16, size * 0.16);
    p.fillPath(path, bg);
    QFont f = p.font();
    f.setBold(true);
    f.setPixelSize(int(size * 0.5));
    p.setFont(f);
    p.setPen(QColor(0x1b, 0x1b, 0x1b));
    p.drawText(QRectF(0, 0, size, size), Qt::AlignCenter, initial);
    p.end();
    cache.insert(key, new QPixmap(pm), int(qMin<qint64>(pm.width() * qint64(pm.height()) * 4, 4 << 20)));
    return pm;
}

/* The picture file of an avatar cut to a square of at most `side` device pixels, rounded up to 32, 64, 96 ...: the sizes a list or a message draws
   (16 to 32 points) share one picture of 4 KB (16 KB at twice the scale). It is decoded straight to that size and kept in a bounded cache. (The whole decode was kept before, 320 pixels square: 400 KB for every person and room whose picture was ever drawn, 1.2 GB for a room
   of 3 000 people.) A null image: the file cannot be read. */
static QImage avatarSquare(const QString &path, int side)
{
    static QCache<QString, QImage> cache(24 << 20);
    const int tier = (qMax(side, 1) + 31) / 32 * 32;
    const QString key = QString::number(tier) + QChar(1) + path;
    if (const QImage *hit = cache.object(key)) return *hit;
    QImage sq;
    const QImage img = loadFitted(path, QSize(tier, tier), Qt::KeepAspectRatioByExpanding); /* never bigger than needed, never enlarged */
    if (!img.isNull()) {
        const int m = qMin(img.width(), img.height());
        sq = img.copy((img.width() - m) / 2, (img.height() - m) / 2, m, m);
    }
    cache.insert(key, new QImage(sq), sq.isNull() ? 1 : int(sq.sizeInBytes()));
    return sq;
}

QPixmap profilePixmap(const QString &imagePath, const QString &seed, const QString &label, int size, qreal dpr, const QPalette &pal, bool *loaded)
{
    if (loaded) *loaded = false;
    const QSize device = QSize(size, size) * dpr;
    const QImage master = imagePath.isEmpty() ? QImage() : avatarSquare(imagePath, device.width());
    if (master.isNull()) return avatarPixmap(seed, label, size, dpr, pal);
    QImage sq = master.size() == device ? master : master.scaled(device, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
    QPixmap pm(device);
    pm.setDevicePixelRatio(dpr);
    pm.fill(Qt::transparent);
    QPainter p(&pm);
    p.setRenderHint(QPainter::Antialiasing);
    QPainterPath path;
    path.addRoundedRect(QRectF(0, 0, size, size), size * 0.16, size * 0.16);
    p.setClipPath(path);
    sq.setDevicePixelRatio(dpr);
    p.drawImage(QRectF(0, 0, size, size), sq);
    if (loaded) *loaded = true;
    return pm;
}

QPixmap withPresenceDot(const QPixmap &avatar, const QString &state, const QPalette &pal)
{
    QColor dot;
    if (state == "online") dot = QColor(0x3f, 0xb9, 0x50);
    else if (state == "unavailable") dot = QColor(0xd2, 0x99, 0x22);
    else if (state == "offline") dot = QColor(0x8b, 0x94, 0x9e);
    else return avatar;
    QPixmap pm = avatar;
    const qreal dpr = pm.devicePixelRatio();
    const qreal w = pm.width() / dpr, h = pm.height() / dpr, r = qMax<qreal>(3.0, w * 0.2);
    QPainter p(&pm);
    p.setRenderHint(QPainter::Antialiasing);
    p.setBrush(pal.color(QPalette::Window));
    p.setPen(Qt::NoPen);
    p.drawEllipse(QPointF(w - r, h - r), r + 1.2, r + 1.2); /* ring in the background colour so the dot reads on any picture */
    p.setBrush(dot);
    p.drawEllipse(QPointF(w - r, h - r), r, r);
    return pm;
}

QPixmap shieldPixmap(int kind, int size, qreal dpr)
{
    QPixmap pm(QSize(size, size) * dpr);
    pm.setDevicePixelRatio(dpr);
    pm.fill(Qt::transparent);
    QPainter p(&pm);
    p.setRenderHint(QPainter::Antialiasing);
    const QColor fill = kind == 0 ? QColor(0x3f, 0xb9, 0x50) : kind == 1 ? QColor(0xd2, 0x99, 0x22) : QColor(0xe5, 0x53, 0x4b);
    const qreal s = size;
    QPainterPath path;
    path.moveTo(s * 0.5, s * 0.04);
    path.lineTo(s * 0.92, s * 0.18);
    path.lineTo(s * 0.92, s * 0.50);
    path.quadTo(s * 0.92, s * 0.82, s * 0.5, s * 0.97);
    path.quadTo(s * 0.08, s * 0.82, s * 0.08, s * 0.50);
    path.lineTo(s * 0.08, s * 0.18);
    path.closeSubpath();
    p.fillPath(path, fill);
    QPen pen(Qt::white, qMax<qreal>(1.2, s * 0.13), Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin);
    p.setPen(pen);
    if (kind == 0) { /* check mark */
        p.drawLine(QPointF(s * 0.30, s * 0.52), QPointF(s * 0.45, s * 0.67));
        p.drawLine(QPointF(s * 0.45, s * 0.67), QPointF(s * 0.72, s * 0.35));
    } else { /* exclamation mark */
        p.drawLine(QPointF(s * 0.5, s * 0.28), QPointF(s * 0.5, s * 0.58));
        p.drawPoint(QPointF(s * 0.5, s * 0.74));
    }
    return pm;
}

}
