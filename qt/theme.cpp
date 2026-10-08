#include "qt/theme.h"
#include <QDir>
#include <QFile>
#include <QPainter>
#include <QPainterPath>
#include <QStyle>
#include <QWidget>
#include <QSettings>
#include <QStandardPaths>
#include <cmath>
#include <QStyle>
#include <QStyleFactory>

namespace vc {

static quint32 fnv(const QByteArray &b)
{
    quint32 h = 2166136261u;
    for (unsigned char c : b) h = (h ^ c) * 16777619u;
    return h;
}

bool isDark(const QPalette &pal) { return pal.color(QPalette::Window).lightness() < 128; }

QColor nameColor(const QString &seed, const QPalette &pal)
{
    int hue = int(fnv(seed.toUtf8()) % 360);
    return isDark(pal) ? QColor::fromHsl(hue, 150, 170) : QColor::fromHsl(hue, 170, 85);
}

Theme savedTheme() { return QSettings("vector", "vector").value("theme", "system").toString() == "dark" ? Theme::Dark : Theme::System; }
void saveTheme(Theme t) { QSettings("vector", "vector").setValue("theme", t == Theme::Dark ? "dark" : "system"); }

static QPalette ripcordDark()
{
    QPalette p;
    const QColor window(0x32, 0x32, 0x32), base(0x2b, 0x2b, 0x2b), text(0xdc, 0xdc, 0xdc), button(0x44, 0x44, 0x44), hi(0x59, 0x68, 0x78);
    p.setColor(QPalette::Window, window);
    p.setColor(QPalette::WindowText, text);
    p.setColor(QPalette::Base, base);
    p.setColor(QPalette::AlternateBase, QColor(0x36, 0x36, 0x36));
    p.setColor(QPalette::Text, text);
    p.setColor(QPalette::Button, button);
    p.setColor(QPalette::ButtonText, text);
    p.setColor(QPalette::ToolTipBase, QColor(0x3a, 0x3a, 0x3a));
    p.setColor(QPalette::ToolTipText, text);
    p.setColor(QPalette::PlaceholderText, QColor(0x8d, 0x8d, 0x8d));
    p.setColor(QPalette::Highlight, hi);
    p.setColor(QPalette::HighlightedText, Qt::white);
    p.setColor(QPalette::Link, QColor(0x6a, 0xa8, 0xe8));
    p.setColor(QPalette::LinkVisited, QColor(0xa9, 0x8b, 0xe8));
    p.setColor(QPalette::Mid, QColor(0x1f, 0x1f, 0x1f));
    p.setColor(QPalette::Dark, QColor(0x1c, 0x1c, 0x1c));
    p.setColor(QPalette::Light, QColor(0x50, 0x50, 0x50));
    p.setColor(QPalette::Disabled, QPalette::Text, QColor(0x80, 0x80, 0x80));
    p.setColor(QPalette::Disabled, QPalette::ButtonText, QColor(0x80, 0x80, 0x80));
    return p;
}

void applyTheme(QApplication &app, Theme t)
{
    static QPalette *systemPalette = nullptr;
    static QString *systemStyle = nullptr;
    if (!systemPalette) { systemPalette = new QPalette(app.palette()); systemStyle = new QString(app.style()->objectName()); }
    if (t == Theme::Dark) {
        app.setStyle(QStyleFactory::create("Fusion"));
        app.setPalette(ripcordDark());
    } else {
        QStyle *s = QStyleFactory::create(*systemStyle);
        if (s) app.setStyle(s);
        app.setPalette(*systemPalette);
    }
    /* Widgets that already exist keep colours resolved from the old palette (item views, the text browser, line edits): let them inherit afresh. */
    for (QWidget *w : app.allWidgets()) {
        w->setPalette(QPalette());
        w->style()->unpolish(w);
        w->style()->polish(w);
        w->update();
    }
}

static QString g_emojiFamily;
QString emojiFamily() { return g_emojiFamily; }
void setEmojiFamily(const QString &f) { g_emojiFamily = f; }


/* a rounded blue square with a white V; with unread messages a red badge with the count */
/* The logo is \vec{v} as TeX sets it: the outlines of "v" (math italic) and the combining vector arrow from Latin Modern Math (GUST Font License),
   embedded as path data (y up, font units) so no font is needed at run time. */
static const char *kVecGlyph =
    "M 468 372 C 468 426 442 442 424 442 C 399 442 375 416 375 394 C 375 381 380 375 391 364 C 412 344 425 318 425 282 C 425 240 364 11 247 11 C 196 11 173 46 173 98 C 173 154 200 227 231 310 C 238 327 243 341 243 360 C 243 405 211 442 161 442 C 67 442 29 297 29 288 C 29 278 41 278 41 278 C 51 278 52 280 57 296 C 86 397 129 420 158 420 C 166 420 183 420 183 388 C 183 363 173 336 166 318 C 122 202 109 156 109 113 C 109 5 197 -11 243 -11 C 411 -11 468 320 468 372 Z "
    "M 456.5 597 C 456.5 604 451.5 609 445.5 611 C 409.5 623 380.5 649 364.5 683 C 362.5 688 357.5 692 351.5 692 C 342.5 692 336.5 685 336.5 677 C 336.5 675 336.5 673 337.5 671 C 347.5 648 363.5 628 382.5 612 L 55.5 612 C 47.5 612 40.5 605 40.5 597 C 40.5 589 47.5 582 55.5 582 L 382.5 582 C 363.5 566 347.5 546 337.5 523 C 336.5 521 336.5 519 336.5 517 C 336.5 509 342.5 502 351.5 502 C 357.5 502 362.5 506 364.5 511 C 380.5 545 409.5 571 445.5 583 C 451.5 585 456.5 590 456.5 597 Z";
static const QRectF kVecBounds(29, -692, 439, 703);

static QPainterPath vecPath()
{
    QPainterPath path;
    const QStringList t = QString(kVecGlyph).split(' ', Qt::SkipEmptyParts);
    for (int i = 0; i < t.size();) {
        const QString c = t[i++];
        auto n = [&] { return t[i++].toDouble(); };
        if (c == "M") { qreal x = n(), y = n(); path.moveTo(x, -y); }
        else if (c == "L") { qreal x = n(), y = n(); path.lineTo(x, -y); }
        else if (c == "C") { qreal a = n(), b = n(), c2 = n(), d = n(), e = n(), f = n(); path.cubicTo(a, -b, c2, -d, e, -f); }
        else if (c == "Z") path.closeSubpath();
    }
    return path;
}

QPixmap trayPixmap(int unread, int size)
{
    QPixmap pm(size, size);
    pm.fill(Qt::transparent);
    QPainter p(&pm);
    p.setRenderHint(QPainter::Antialiasing);
    const qreal s = size;
    QLinearGradient g(0, 0, 0, s);
    g.setColorAt(0, QColor(0x4f, 0x8c, 0xd4));
    g.setColorAt(1, QColor(0x2c, 0x5a, 0x94));
    p.setBrush(g);
    p.setPen(Qt::NoPen);
    p.drawRoundedRect(QRectF(s * 0.04, s * 0.04, s * 0.92, s * 0.92), s * 0.2, s * 0.2);
    {
        QPainterPath vp = vecPath();
        const qreal h = s * 0.62, k = h / kVecBounds.height();
        p.save();
        p.translate(s / 2 - kVecBounds.center().x() * k, s / 2 - kVecBounds.center().y() * k);
        p.scale(k, k);
        p.setPen(Qt::NoPen);
        p.setBrush(Qt::white);
        p.drawPath(vp);
        p.restore();
    }
    QFont f = p.font();
    f.setBold(true);
    if (unread > 0) {
        const QString t = unread > 99 ? "99+" : QString::number(unread);
        const qreal d = s * (t.size() > 2 ? 0.62 : t.size() > 1 ? 0.52 : 0.42);
        QRectF badge(s - d, 0, d, s * 0.42);
        p.setBrush(QColor(0xe5, 0x53, 0x4b));
        p.setPen(QPen(Qt::white, s * 0.03));
        p.drawRoundedRect(badge, s * 0.2, s * 0.2);
        f.setPixelSize(int(s * 0.28));
        p.setFont(f);
        p.setPen(Qt::white);
        p.drawText(badge, Qt::AlignCenter, t);
    }
    return pm;
}

static QString g_dateFormat, g_timeFormat;

static void loadTimestampFormats()
{
    if (!g_dateFormat.isEmpty()) return;
    QSettings st("vector", "vector");
    g_dateFormat = st.value("date_format", "dd/MM/yyyy").toString();
    g_timeFormat = st.value("time_format", "HH:mm").toString();
    if (g_dateFormat.trimmed().isEmpty()) g_dateFormat = "dd/MM/yyyy";
    if (g_timeFormat.trimmed().isEmpty()) g_timeFormat = "HH:mm";
}

QString dateFormat() { loadTimestampFormats(); return g_dateFormat; }
QString timeFormat() { loadTimestampFormats(); return g_timeFormat; }

void setTimestampFormats(const QString &date, const QString &time)
{
    g_dateFormat = date.trimmed().isEmpty() ? QString("dd/MM/yyyy") : date;
    g_timeFormat = time.trimmed().isEmpty() ? QString("HH:mm") : time;
    QSettings st("vector", "vector");
    st.setValue("date_format", g_dateFormat);
    st.setValue("time_format", g_timeFormat);
}

QString formatWhen(const QDateTime &dt, bool allDates, bool withSeconds)
{
    QString t = timeFormat();
    if (withSeconds) { const int at = t.indexOf("mm"); if (at >= 0 && !t.contains("ss")) t.insert(at + 2, ":ss"); }
    if (!allDates && dt.date() == QDate::currentDate()) return dt.toString(t);
    return dt.toString(dateFormat()) + " " + dt.toString(t);
}

QIcon appIcon()
{
    QIcon i;
    for (int sz : {16, 24, 32, 48, 64, 128, 256}) i.addPixmap(trayPixmap(0, sz));
    return i;
}

QString notificationSoundFile()
{
    const QString dir = QStandardPaths::writableLocation(QStandardPaths::CacheLocation);
    QDir().mkpath(dir);
    const QString path = dir + "/notify.wav";
    if (QFile::exists(path)) return path;
    const int rate = 44100, n = int(rate * 0.42);
    QByteArray pcm;
    pcm.resize(n * 2);
    auto *out = reinterpret_cast<qint16 *>(pcm.data());
    for (int i = 0; i < n; i++) { /* two descending notes with a soft decay */
        const double t = double(i) / rate, f = t < 0.18 ? 1046.5 : 784.0, local = t < 0.18 ? t : t - 0.18;
        const double env = std::exp(-local * 11.0) * std::min(1.0, local * 400.0);
        out[i] = qint16(std::sin(2 * M_PI * f * t) * env * 0.35 * 32767);
    }
    QByteArray wav;
    auto le32 = [&](quint32 v) { for (int k = 0; k < 4; k++) wav.append(char((v >> (8 * k)) & 0xff)); };
    auto le16 = [&](quint16 v) { wav.append(char(v & 0xff)); wav.append(char(v >> 8)); };
    wav.append("RIFF"); le32(36 + quint32(pcm.size())); wav.append("WAVEfmt "); le32(16); le16(1); le16(1); le32(rate); le32(rate * 2); le16(2); le16(16);
    wav.append("data"); le32(quint32(pcm.size())); wav.append(pcm);
    QFile f(path);
    if (!f.open(QIODevice::WriteOnly) || f.write(wav) != wav.size()) return QString();
    return path;
}

}
