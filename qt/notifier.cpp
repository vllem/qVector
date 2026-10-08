#include "qt/notifier.h"
#include <QSystemTrayIcon>
#include <QVariantMap>
#include <cstdio>
#ifdef Q_OS_LINUX
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusInterface>
#include <QDBusReply>
#endif

namespace vc {

#ifdef Q_OS_LINUX

static const char *SERVICE = "org.freedesktop.Notifications", *PATH = "/org/freedesktop/Notifications", *IFACE = "org.freedesktop.Notifications";

Notifier::Notifier(QObject *parent) : QObject(parent)
{
    QDBusConnection bus = QDBusConnection::sessionBus();
    available_ = bus.isConnected() && bus.interface() && bus.interface()->isServiceRegistered(SERVICE);
    if (!available_) { fprintf(stderr, "notify: no notification service on the session bus; popups are disabled\n"); return; }
    bus.connect(SERVICE, PATH, IFACE, "ActionInvoked", this, SLOT(onAction(uint, QString)));
    bus.connect(SERVICE, PATH, IFACE, "NotificationClosed", this, SLOT(onClosed(uint, uint)));
}

void Notifier::show(const QString &roomId, const QString &title, const QString &body, bool highlight)
{
    if (!available_) return;
    QDBusInterface iface(SERVICE, PATH, IFACE, QDBusConnection::sessionBus());
    QVariantMap hints;
    hints.insert("urgency", QVariant::fromValue<uchar>(highlight ? 2 : 1));
    hints.insert("desktop-entry", "vector");
    uint replaces = byRoom_.value(roomId, 0);
    QDBusReply<uint> r = iface.call("Notify", "qVector", replaces, "mail-unread", title, body, QStringList{"default", "Open"}, hints, highlight ? 0 : 8000);
    if (!r.isValid()) { fprintf(stderr, "notify: %s\n", r.error().message().toUtf8().constData()); return; }
    rooms_.insert(r.value(), roomId);
    byRoom_.insert(roomId, r.value());
}

void Notifier::onAction(uint id, const QString &key)
{
    if (key != "default" || !rooms_.contains(id)) return;
    emit activated(rooms_.value(id));
}

void Notifier::onClosed(uint id, uint)
{
    QString room = rooms_.take(id);
    if (!room.isEmpty() && byRoom_.value(room) == id) byRoom_.remove(room);
}

void Notifier::setTray(QSystemTrayIcon *tray) { Q_UNUSED(tray); }

#else  /* Windows, macOS: the tray icon shows the message, a click on it opens the room */

Notifier::Notifier(QObject *parent) : QObject(parent) {}

void Notifier::setTray(QSystemTrayIcon *tray)
{
    tray_ = tray;
    available_ = tray != nullptr && QSystemTrayIcon::supportsMessages();
    if (tray_) connect(tray_, &QSystemTrayIcon::messageClicked, this, [this] { if (!lastRoom_.isEmpty()) emit activated(lastRoom_); });
}

void Notifier::show(const QString &roomId, const QString &title, const QString &body, bool highlight)
{
    if (!available_ || !tray_ || !tray_->isVisible()) return;
    lastRoom_ = roomId;
    tray_->showMessage(title, body, highlight ? QSystemTrayIcon::Warning : QSystemTrayIcon::Information, highlight ? 15000 : 8000);
}

void Notifier::onAction(uint, const QString &) {}
void Notifier::onClosed(uint, uint) {}

#endif

}
