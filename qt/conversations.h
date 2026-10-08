#ifndef VC_QT_CONVERSATIONS_H
#define VC_QT_CONVERSATIONS_H

#include "qt/core.h"
#include <QCheckBox>
#include <QDialog>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QTimer>

namespace vc {

/* Start a direct message: type a name or a Matrix id, pick from the server's user directory (engine event "users"). */
class StartDmDialog : public QDialog {
    Q_OBJECT
public:
    StartDmDialog(Core *core, QWidget *parent);
    void setUsers(const QJsonArray &users);
private:
    QString chosen() const;
    Core *core_;
    QLineEdit *who_;
    QListWidget *hits_;
    QPushButton *go_;
    QTimer debounce_;
    QJsonArray users_;
    void rebuild();
};

class CreateRoomDialog : public QDialog {
    Q_OBJECT
public:
    CreateRoomDialog(Core *core, QWidget *parent);
};

/* Browse / search the public room directory of a server, and join (engine event "directory"). */
class BrowseRoomsDialog : public QDialog {
    Q_OBJECT
public:
    BrowseRoomsDialog(Core *core, QWidget *parent, const QString &spaceId = QString()); /* a space id lists that space's rooms (engine event "space_rooms") instead of the public directory */
    void setRooms(const QJsonArray &rooms);
private:
    void search();
    void joinSelected();
    Core *core_;
    QString spaceId_;
    QLineEdit *term_, *server_;
    QListWidget *list_;
    QLabel *status_;
    QPushButton *join_;
};

/* choose the rooms to send a copy of a message to */
class ForwardDialog : public QDialog {
    Q_OBJECT
public:
    ForwardDialog(Core *core, const QJsonArray &rooms, const QString &eventId, QWidget *parent);
};

/* every version of an edited message, oldest first (engine event "edit_history") */
void showEditHistory(QWidget *parent, const QJsonArray &revisions);

}

#endif
