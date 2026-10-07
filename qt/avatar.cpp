#include "qt/avatar.h"
#include "qt/theme.h"
#include <QHash>
#include <QImage>
#include <QImageReader>
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
    return pm;
}

QPixmap profilePixmap(const QString &imagePath, const QString &seed, const QString &label, int size, qreal dpr, const QPalette &pal, bool *loaded)
{
    if (loaded) *loaded = false;
    static QHash<QString, QImage> decoded; /* by file: avatars are drawn many times */
    QImage img;
    if (!imagePath.isEmpty()) {
        auto it = decoded.constFind(imagePath);
        if (it == decoded.cend()) { QImageReader rd(imagePath); rd.setAutoTransform(true); it = decoded.insert(imagePath, rd.read()); }
        img = it.value();
    }
    if (img.isNull()) return avatarPixmap(seed, label, size, dpr, pal);
    const QImage *src = &img;
    QImage sq = src->scaled(QSize(size, size) * dpr, Qt::KeepAspectRatioByExpanding, Qt::SmoothTransformation);
    sq = sq.copy((sq.width() - int(size * dpr)) / 2, (sq.height() - int(size * dpr)) / 2, int(size * dpr), int(size * dpr));
    QPixmap pm(QSize(size, size) * dpr);
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
