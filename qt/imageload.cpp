#include "qt/imageload.h"
#include <QDir>
#include <QFileInfo>
#include <QImageReader>
#ifdef __GLIBC__
#include <malloc.h>
#endif

namespace vc {

void giveBackFreedMemory()
{
#ifdef __GLIBC__
    malloc_trim(0);
#endif
}

QImage loadFitted(const QString &path, const QSize &box, Qt::AspectRatioMode mode)
{
    if (path.isEmpty()) return QImage();
    QImageReader rd(path);
    rd.setAutoTransform(true);
    QSize want; /* the size of the picture as it will be shown (after the orientation), when smaller than the file's */
    const QSize full = rd.size(); /* as stored: a photo taken sideways is rotated after it has been read */
    if (full.isValid() && !box.isEmpty()) {
        const bool turned = rd.transformation() & QImageIOHandler::TransformationRotate90;
        const QSize shown = turned ? full.transposed() : full;
        const QSize fit = shown.scaled(box, mode);
        if (fit.width() < shown.width() && fit.height() < shown.height()) {
            want = fit;
            rd.setScaledSize(turned ? fit.transposed() : fit);
        }
    }
    QImage img = rd.read();
    /* a format that could not tell its size, or ignores the scaled size, still ends up no bigger than asked */
    if (!img.isNull() && !box.isEmpty() && !want.isValid()) {
        const QSize fit = img.size().scaled(box, mode);
        if (fit.width() < img.width() && fit.height() < img.height()) img = img.scaled(fit, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
    }
    return img;
}

/* does the picture really use its transparency? (a screenshot from the clipboard has an alpha channel and is opaque) */
static bool usesAlpha(const QImage &img)
{
    if (!img.hasAlphaChannel()) return false;
    const QImage a = img.convertToFormat(QImage::Format_ARGB32);
    for (int y = 0; y < a.height(); y++) {
        const QRgb *line = reinterpret_cast<const QRgb *>(a.constScanLine(y));
        for (int x = 0; x < a.width(); x++) if (qAlpha(line[x]) != 255) return true;
    }
    return false;
}

PictureInfo pictureInfo(const QString &path, const QString &thumbDir, const QString &thumbName)
{
    static const qint64 kSmall = 256 * 1024; /* as in the engine (ui.rs SMALL_PICTURE): a picture this small is fetched whole, a thumbnail would hardly be smaller */
    static const QSize kBox(960, 640); /* what the timeline draws a picture at on a screen with twice the pixels (480x320 points): a smaller thumbnail is enlarged there and looks soft */
    PictureInfo out;
    const QFileInfo fi(path);
    const QString ext = fi.suffix().toLower();
    if (ext != "png" && ext != "jpg" && ext != "jpeg" && ext != "gif" && ext != "webp") return out;
    QImageReader rd(path);
    rd.setAutoTransform(true);
    const QSize stored = rd.size();
    if (!stored.isValid()) return out;
    const bool turned = rd.transformation() & QImageIOHandler::TransformationRotate90;
    const QSize shown = turned ? stored.transposed() : stored;
    out.width = shown.width();
    out.height = shown.height();
    if (ext == "gif" || fi.size() <= kSmall || (shown.width() <= kBox.width() && shown.height() <= kBox.height())) return out;
    const QImage img = loadFitted(path, kBox);
    if (img.isNull()) return out;
    const bool alpha = usesAlpha(img);
    const QString file = QDir(thumbDir).filePath(thumbName + (alpha ? ".png" : ".jpg"));
    const bool saved = alpha ? img.save(file, "PNG") : img.convertToFormat(QImage::Format_RGB32).save(file, "JPEG", 82);
    if (!saved) return out;
    out.thumbPath = file;
    out.thumbMime = alpha ? "image/png" : "image/jpeg";
    out.thumbWidth = img.width();
    out.thumbHeight = img.height();
    giveBackFreedMemory(); /* a PNG was decoded in full: megabytes that are gone now */
    return out;
}

}
