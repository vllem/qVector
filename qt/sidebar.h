#ifndef VC_QT_SIDEBAR_H
#define VC_QT_SIDEBAR_H

#include <QHash>
#include <QJsonArray>
#include <QSet>
#include <QTreeWidget>

namespace vc {

/* Ripcord's left pane: workspace header, then collapsible sections (Invitations, Favourites, Direct Messages, spaces, Rooms) with unread badges.
   The rows are the engine's room list (core-rs `UiRoom` as JSON): the engine decides what is shown and in which order, this only draws it. */
class Sidebar : public QTreeWidget {
    Q_OBJECT
public:
    explicit Sidebar(QWidget *parent = nullptr);
    void refresh(const QJsonArray &rooms, const QString &current, const QString &workspace);
    void setCurrent(const QString &id) { refresh(rooms_, id, workspace_); }
protected:
    void changeEvent(QEvent *e) override;
signals:
    void roomActivated(const QString &roomId);
    void leaveRequested(const QString &roomId);
    void exploreRequested(const QString &spaceId); /* "Explore rooms..." on a space's section header */
    void notifyRequested(const QString &roomId, const QString &level); /* "default", "all", "mentions" or "mute" */
    void tagRequested(const QString &roomId, const QString &kind); /* "favourite", "low_priority" or "none" */

private:
    QJsonArray rooms_;
    QString sig_, shape_, current_, workspace_;
    QRgb paletteKey_ = 0;
    QSet<QString> collapsed_;
    struct RowInfo { bool favourite, lowpriority, invite; QString notify; };
    QHash<QString, RowInfo> rows_; /* what the last refresh showed: the context menu reads it */
};

}

#endif
