#ifndef VC_QT_NOTIFIER_H
#define VC_QT_NOTIFIER_H

#include <QHash>
#include <QObject>
#include <QString>

class QSystemTrayIcon;

namespace vc {

/* Desktop notifications. Linux: the freedesktop.org Notifications service (D-Bus). Windows and macOS: balloon / notification-centre messages of the
   tray icon (setTray). Clicking a notification asks for its room to be opened. */
class Notifier : public QObject {
    Q_OBJECT
public:
    explicit Notifier(QObject *parent = nullptr);
    bool available() const { return available_; }
    void show(const QString &roomId, const QString &title, const QString &body, bool highlight);
    void setTray(QSystemTrayIcon *tray); /* where notifications appear on platforms without D-Bus */
signals:
    void activated(const QString &roomId);

private slots:
    void onAction(uint id, const QString &key);
    void onClosed(uint id, uint reason);

private:
    bool available_ = false;
    QSystemTrayIcon *tray_ = nullptr;
    QString lastRoom_;
    QHash<uint, QString> rooms_;     /* notification id -> room */
    QHash<QString, uint> byRoom_;    /* room -> its latest notification id, replaced rather than stacked */
};

}

#endif
