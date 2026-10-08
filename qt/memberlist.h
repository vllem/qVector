#ifndef VC_QT_MEMBERLIST_H
#define VC_QT_MEMBERLIST_H

#include <QJsonObject>
#include <QListWidget>

namespace vc {

/* The people in the open room (from the engine's room details): staff first, then members, then the banned. */
class MemberList : public QListWidget {
    Q_OBJECT
public:
    explicit MemberList(QWidget *parent = nullptr);
    void setMe(const QString &userId) { me_ = userId; }
    void refresh(const QJsonObject &details);
    void setPresence(const QJsonObject &states); /* user id -> "online" | "unavailable" | "offline" (the engine's `presence` event) */
signals:
    void moderationRequested(const QString &action, const QString &userId); /* "kick", "ban", "unban", "role:admin" / "role:moderator" / "role:member" */
    void verifyRequested(const QString &userId);  /* "Verify...": compare emoji with this person */
    void messageRequested(const QString &userId); /* "Message" in the context menu: start / open a direct message */

private:
    QString sig_, me_;
    QJsonObject presence_, last_;
    bool canSetRoles_ = false;
};

}

#endif
