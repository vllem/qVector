#ifndef VC_QT_MAINWINDOW_H
#define VC_QT_MAINWINDOW_H

#include "qt/conversations.h"
#include "qt/core.h"
#include "qt/dialogs.h"
#include "qt/memberlist.h"
#include "qt/searchpanel.h"
#include "qt/emojipicker.h"
#include "qt/loginpage.h"
#include "qt/notifier.h"
#include "qt/sidebar.h"
#include "qt/timelineview.h"
#include <QAction>
#include <QCloseEvent>
#include <QHash>
#include <QImage>
#include <QTemporaryDir>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMainWindow>
#include <QFrame>
#include <QPointer>
#include <QPushButton>
#include <QSet>
#include <QSplitter>
#include <QStackedWidget>
#include <QSystemTrayIcon>
#include <QTabBar>
#include <QToolButton>
#include <functional>

namespace vc {

/* The message line; Ctrl+V with a picture on the clipboard (and no text) offers to send the picture. */
class Composer : public QLineEdit {
    Q_OBJECT
public:
    struct Candidate { QString id, label, insert; }; /* insert: what replaces the typed word (default: the label and a space) */
    explicit Composer(QWidget *parent = nullptr);
    ~Composer() override;
    std::function<QList<Candidate>(const QString &prefix)> candidates; /* people (and @room) matching what follows an '@' */
signals:
    void imagePasted(const QImage &img);
    void filesPasted(const QStringList &paths);
    void escapePressed();
    void mentionPicked(const QString &userId, const QString &label);
protected:
    void keyPressEvent(QKeyEvent *e) override;
    void focusOutEvent(QFocusEvent *e) override;
private:
    void updatePopup();
    void acceptCandidate();
    QListWidget *popup_;
    QList<Candidate> shown_;
    int tokenStart_ = -1;
};

class MainWindow : public QMainWindow {
    Q_OBJECT
public:
    MainWindow(Core *core, bool demo);
    ~MainWindow() override;
    void start();
    void openRoomByTitle(const QString &title); /* dev aid: opens the first room with this name */
    void dialogForDemo(const QString &which);   /* dev aid */
    void membersForDemo() { toggleMembers(); }
    void setAutoConfirm(bool on) { autoConfirm_ = on; } /* dev aid: confirm the emoji by itself */
    void verifyForDemo() { core_->call("request_verification"); }
    void threadForDemo(const QString &text); /* opens the thread of the first message containing this text */
    void searchForDemo(const QString &term) { toggleSearch(); searchPanel_->setTerm(term); }
    Core *core() const { return core_; }

private:
    void onEvent(const QString &name, const QJsonValue &payload);
    void buildMenus();
    void openRoom(const QString &id);
    void closeTab(int index);
    int tabIndex(const QString &id) const;
    void updateTitle();
    void updateStatus();
    void updateTopic();
    void updateTabs();
    void showMain(bool main);
    void send();
    void cancelContext();
    void showContextBar(const QString &text);
    void startReply(const QString &eventId);
    void startEdit(const QString &eventId);
    void chooseFiles();
    void stageFiles(const QStringList &paths);
    void stageImage(const QImage &img);
    void cancelAttachment();
    void saveAttachment(const QString &eventId);
    void toggleMembers();
    void toggleSearch();
    void layoutPanes();
    void openThread(const QString &rootId);
    void closeThread();
    void sendThreadReply();
    void updatePinBar();
    void showPreferences();
    void showSaved();
    void showVerify(const QJsonObject &state);
    void showRecovery(bool create);
    void roomSettings();
    void verifyPerson(const QString &userId);
    void jumpTo(const QString &room, const QString &event);
    void openPicture(const QString &eventId);
    void saveDrafts();
    void loadDrafts();
    QString roomTitle(const QString &id) const;
    QJsonObject roomRow(const QString &id) const;
    bool event(QEvent *e) override;
    void closeEvent(QCloseEvent *e) override;
    void dragEnterEvent(QDragEnterEvent *e) override;
    void dropEvent(QDropEvent *e) override;

    Core *core_;
    bool demo_;
    QStackedWidget *root_;
    LoginPage *login_;
    Sidebar *sidebar_;
    QSplitter *split_;
    QTabBar *tabs_;
    QFrame *statusLine_ = nullptr;
    void applyChrome(); /* divider/composer colours and tab style follow the current palette and style (system Qt themes) */
    QToolButton *plus_;
    QLabel *topic_, *banner_, *statusLeft_, *statusRight_, *typingLabel_;
    QToolButton *membersBtn_;
    TimelineView *timeline_;
    QWidget *empty_, *inviteBar_, *contextBar_;
    QLabel *contextLabel_;
    QWidget *attachBar_;
    QLabel *attachThumb_, *attachLabel_;
    QStringList pending_;
    QTemporaryDir *pasteDir_ = nullptr;
    QStackedWidget *timelines_;
    Composer *composer_;
    QToolButton *attach_, *emojiBtn_;
    EmojiPicker *picker_, *reactPicker_;
    MemberList *memberList_;
    SearchPanel *searchPanel_;
    QWidget *threadPanel_, *pinBar_;
    QLabel *threadTitle_;
    QLineEdit *threadInput_;
    TimelineView *threadView_;
    QPushButton *pinText_, *pinPrev_, *pinNext_, *pinOff_;
    int pinIndex_ = 0;
    QList<QJsonObject> pinned_;
    QSet<QString> bookmarkIds_;
    QJsonArray bookmarks_;
    QPointer<SavedDialog> savedDlg_;
    QPointer<VerifyDialog> verifyDlg_;
    QPointer<RecoveryDialog> recoveryDlg_;
    QPointer<StartDmDialog> startDm_;
    QPointer<BrowseRoomsDialog> browse_;
    QAction *actDark_ = nullptr, *actNotify_ = nullptr, *actTyping_ = nullptr, *actPreviews_ = nullptr, *actIndex_ = nullptr, *actTray_ = nullptr, *actCloseTray_ = nullptr;
    QJsonArray sessions_;
    QPointer<QDialog> prefsDlg_;
    QString pendingReveal_;
    bool sendTyping_ = true, autoConfirm_ = false;
    Notifier *notifier_ = nullptr;
    QSystemTrayIcon *tray_ = nullptr;
    QString current_, replyTo_, editing_, reactEvent_;
    QString user_, workspace_;
    QJsonArray rooms_;
    QJsonObject details_, session_;
    QHash<QString, QString> drafts_;
    bool notify_ = true;
};

}

#endif
