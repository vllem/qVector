#include "qt/core.h"
#include <QJsonDocument>
#include <QMetaObject>
#include <QCoreApplication>
#include <QPointer>
extern "C" {
#include "../core-rs/vector_app.h"
}

namespace vc {

/* engine threads call this: hand the event to the GUI thread */
void Core::trampoline(void *user, const char *name, const char *json)
{
    Core *self = static_cast<Core *>(user); /* ~Core stops the engine first, so no event arrives after the object is gone */
    const QString n = QString::fromUtf8(name);
    const QByteArray raw(json);
    QMetaObject::invokeMethod(self, [self, n, raw] {
        QJsonParseError err;
        const QJsonDocument doc = QJsonDocument::fromJson(raw, &err);
        QJsonValue v;
        if (err.error == QJsonParseError::NoError) v = doc.isArray() ? QJsonValue(doc.array()) : doc.isObject() ? QJsonValue(doc.object()) : QJsonValue();
        else v = QString::fromUtf8(raw); /* a plain string payload (notice texts, file paths) */
        if (doc.isNull() && err.error == QJsonParseError::NoError) v = QJsonValue(QJsonValue::Null);
        emit self->event(n, v);
    }, Qt::QueuedConnection);
}

Core::Core(const QString &dataDir, QObject *parent) : QObject(parent)
{
    app_ = vcr_app_new(dataDir.toUtf8().constData(), &Core::trampoline, this);
    vcr_video_set_sink(app_, &Core::pictureTrampoline, this);
}

Core::~Core()
{
    vcr_app_free(app_); /* stops the engine's threads: no event arrives after this */
    app_ = nullptr;
}

/* a decoder thread calls this; the buffer is only valid now, so copy the picture */
void Core::pictureTrampoline(void *user, int width, int height, const unsigned char *rgba)
{
    Core *self = static_cast<Core *>(user);
    const QImage img = QImage(rgba, width, height, width * 4, QImage::Format_RGBA8888).copy();
    QMetaObject::invokeMethod(self, [self, img] { emit self->remoteFrame(img); }, Qt::QueuedConnection);
}

void Core::pushVideo(const QImage &picture)
{
    if (!app_ || picture.isNull()) return;
    const QImage img = picture.format() == QImage::Format_RGBA8888 ? picture : picture.convertToFormat(QImage::Format_RGBA8888);
    if (img.bytesPerLine() != img.width() * 4) return; /* the engine wants rows without padding */
    vcr_video_push(app_, img.width(), img.height(), img.constBits());
}

QJsonValue Core::call(const char *method, const QJsonObject &args)
{
    if (!app_) return QJsonValue();
    char *out = vcr_app_call(app_, method, QJsonDocument(args).toJson(QJsonDocument::Compact).constData());
    if (!out) return QJsonValue();
    const QByteArray raw(out);
    vcr_string_free(out);
    const QJsonDocument doc = QJsonDocument::fromJson("[" + raw + "]"); /* a bare JSON value is not a document: wrap it */
    const QJsonArray a = doc.array();
    return a.isEmpty() ? QJsonValue() : a.first();
}

}
