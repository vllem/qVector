#ifndef VC_QT_CORE_H
#define VC_QT_CORE_H

#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QHash>
#include <QJsonValue>
#include <QMutex>
#include <QObject>
#include <QString>

struct App; /* the Rust engine (core-rs/vector_app.h) */

namespace vc {

/* The Rust application engine as a Qt object: `call` starts work (or answers directly), results arrive as the `event` signal on the GUI thread. */
class Core : public QObject {
    Q_OBJECT
public:
    explicit Core(const QString &dataDir, QObject *parent = nullptr);
    ~Core() override;
    QJsonValue call(const char *method, const QJsonObject &args = QJsonObject());
    QString pref(const QString &key) { return call("pref", {{"key", key}}).toString(); }
    void setPref(const QString &key, const QString &value) { call("set_pref", {{"key", key}, {"value", value}}); }
    /* Video calls: a camera picture goes to the engine (RGBA8888 or converted); the other side's pictures come back as remoteFrame. */
    void pushVideo(const QImage &picture);
    bool boolPref(const QString &key, bool dflt) { const QString v = pref(key); return v.isEmpty() ? dflt : v == "1"; }
    void setBoolPref(const QString &key, bool on) { setPref(key, on ? "1" : "0"); }

signals:
    void event(const QString &name, const QJsonValue &payload);
    void remoteFrame(const QString &who, const QImage &picture); /* who: user id in a group call, empty in a one-to-one call */

public:
    /* the engine's decoder threads call this with the other side's pictures (public so that the stress test can play the engine) */
    static void pictureTrampoline(void *user, const char *who, int width, int height, const unsigned char *rgba);

private:
    static void trampoline(void *user, const char *name, const char *json);
    ::App *app_ = nullptr;
    QMutex frameLock_;
    QHash<QString, QImage> newestFrames_; /* the newest picture of each person that has not been shown yet: a slow GUI thread skips older ones instead of queueing them */
    bool frameDeliveryQueued_ = false;
};

}

#endif
