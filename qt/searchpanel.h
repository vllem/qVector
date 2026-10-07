#ifndef VC_QT_SEARCHPANEL_H
#define VC_QT_SEARCHPANEL_H

#include <QJsonArray>
#include <QComboBox>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QTimer>
#include <QWidget>

namespace vc {

/* Message search: type, see hits from this room or every room, double-click (or Enter) to jump to the message. */
class SearchPanel : public QWidget {
    Q_OBJECT
public:
    explicit SearchPanel(QWidget *parent = nullptr);
    void setCurrentRoom(const QString &roomId) { room_ = roomId; }
    void focusInput();
    void setTerm(const QString &t) { input_->setText(t); search(); } /* dev aid and "search for this" */
    void setResults(const QJsonArray &hits); /* the engine's answer: [{id, sender, body, time, room_id?, room?}] */
    void setIndexEnabled(bool on) { indexOn_ = on; }
signals:
    void searchRequested(const QString &term, bool allRooms);
    void jumpRequested(const QString &roomId, const QString &eventId);
    void closeRequested();
private:
    void search();
    QString room_;
    bool indexOn_ = false;
    QLineEdit *input_;
    QComboBox *scope_;
    QListWidget *list_;
    QLabel *status_;
    QTimer debounce_;
};

}

#endif
