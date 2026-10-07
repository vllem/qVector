#ifndef VC_QT_CORE_H
#define VC_QT_CORE_H

#include <QJsonArray>
#include <QJsonObject>
#include <QJsonValue>
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
    bool boolPref(const QString &key, bool dflt) { const QString v = pref(key); return v.isEmpty() ? dflt : v == "1"; }
    void setBoolPref(const QString &key, bool on) { setPref(key, on ? "1" : "0"); }

signals:
    void event(const QString &name, const QJsonValue &payload);

private:
    static void trampoline(void *user, const char *name, const char *json);
    ::App *app_ = nullptr;
};

}

#endif
