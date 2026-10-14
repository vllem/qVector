#include "qt/imageload.h"
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

}
