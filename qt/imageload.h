#ifndef VC_QT_IMAGELOAD_H
#define VC_QT_IMAGELOAD_H

#include <QImage>
#include <QSize>
#include <QString>

namespace vc {

/* Give the memory the process has freed (and glibc keeps, free, in its heap) back to the system: a decoded picture's buffers are megabytes that are gone a
   moment later, and glibc, which raises the size above which a block gets a mapping of its own to the biggest it has freed, then leaves them in the heap
   until the next trim (25 MB after ten rooms of photos). Called once after a burst of decoding; a few milliseconds. Does nothing off glibc. */
void giveBackFreedMemory();

/* A picture file decoded no bigger than it will be drawn, EXIF orientation applied. `box` is the size it will be drawn at (device pixels);
   the result is the file's picture scaled into it with `mode` (Qt::KeepAspectRatio: fits inside, Qt::KeepAspectRatioByExpanding: covers it)
   when that is smaller than the file's own size, else the picture as it is (it is never enlarged: the caller scales it to its final size).
   JPEG and WebP are decoded at that size, so a 50-megapixel photo never exists in memory; PNG and the rest are decoded in full and shrunk at
   once. An unreadable file gives a null image. */
QImage loadFitted(const QString &path, const QSize &box, Qt::AspectRatioMode mode = Qt::KeepAspectRatio);

}

#endif
