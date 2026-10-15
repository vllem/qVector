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

/* What the engine is told about a picture that is sent (`pictures` of `send_files`, core-rs/src/app.rs): its size in pixels as it is shown (the EXIF orientation applied)
   and, when it is big, a thumbnail that is made here because the engine has no image decoders. Receivers show the thumbnail instead of downloading the whole file, and
   for a picture in an encrypted room it is the only small version there is (the server cannot scale what it cannot read). */
struct PictureInfo {
    int width = 0, height = 0;                     /* 0: not a picture the engine sends as one */
    QString thumbPath, thumbMime;                  /* empty: no thumbnail */
    int thumbWidth = 0, thumbHeight = 0;
};

/* The size of the picture in `path` and, for a JPEG, PNG or WebP of more than 256 KB that does not fit in 800x600, a thumbnail of it: at most 800x600 (what Element makes and the
   biggest thumbnail a Synapse has by default), JPEG at quality 82, PNG when the picture uses transparency, written to `thumbDir`/`thumbName`.jpg or .png. JPEG and WebP are
   decoded at the thumbnail's size, never in full. No thumbnail for a small file, for a GIF (it would be a still picture) and for what cannot be read; only the file types the
   engine sends as pictures (png, jpg, jpeg, gif, webp) get an answer at all. Safe to call from a thread. */
PictureInfo pictureInfo(const QString &path, const QString &thumbDir, const QString &thumbName);

}

#endif
