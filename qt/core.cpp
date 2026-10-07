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
}

Core::~Core()
{
    vcr_app_free(app_); /* stops the engine's threads: no event arrives after this */
    app_ = nullptr;
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
