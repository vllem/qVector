#ifndef VC_QT_DIALOGS_H
#define VC_QT_DIALOGS_H

#include "qt/core.h"
#include <QDialog>
#include <QElapsedTimer>
#include <QTimer>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QScrollArea>
#include <QVBoxLayout>

class QCamera;
class QMediaCaptureSession;
class QVideoSink;

namespace vc {

/* The messages the user saved, across rooms (engine event "bookmarks") */
class SavedDialog : public QDialog {
    Q_OBJECT
public:
    SavedDialog(Core *core, QWidget *parent);
    void setBookmarks(const QJsonArray &list);
signals:
    void showRequested(const QString &roomId, const QString &eventId);
private:
    Core *core_;
    QListWidget *list_;
};

/* Asks for a question and its answers, then sends the poll to the open room */
class PollDialog : public QDialog {
    Q_OBJECT
public:
    PollDialog(Core *core, QWidget *parent);
};

/* The account's recovery key: unlock this session's secrets with it, or create one (with the account password when the server asks).
   Fed by the engine's "recovery" events. */
class RecoveryDialog : public QDialog {
    Q_OBJECT
public:
    RecoveryDialog(Core *core, bool create, QWidget *parent);
    void setState(const QJsonObject &s);
private:
    Core *core_;
    bool create_;
    QLineEdit *key_, *password_;
    QLabel *status_, *info_;
    QPushButton *go_, *copy_;
    QLineEdit *shown_;
};

/* SAS verification (engine event "verification"): this session against another of our own, or another person */
class VerifyDialog : public QDialog {
    Q_OBJECT
public:
    VerifyDialog(Core *core, QWidget *parent);
    void setState(const QJsonObject &s);
protected:
    void closeEvent(QCloseEvent *) override;
private:
    Core *core_;
    QVBoxLayout *lay_;
    QString me_;
    bool finished_ = false;
};

/* A voice call (engine event "call"): ringing, calling, connected; answer, mute and hang up */
/* The pictures of a video call: the other side large (or a note when there is none), our own camera small in the corner. */
class CallVideo : public QWidget {
    Q_OBJECT
public:
    explicit CallVideo(QWidget *parent = nullptr);
    void setRemote(const QImage &img) { remote_ = img; update(); }
    void setLocal(const QImage &img) { local_ = img; update(); }
    void setNote(const QString &note) { note_ = note; update(); }
    QSize sizeHint() const override { return QSize(560, 360); }
protected:
    void paintEvent(QPaintEvent *) override;
private:
    QImage remote_, local_;
    QString note_;
};

class CallDialog : public QDialog {
    Q_OBJECT
public:
    CallDialog(Core *core, QWidget *parent);
    ~CallDialog() override;
    void setState(const QJsonObject &s);
    /* demo and tests: pictures without a camera */
    CallVideo *video() const { return video_; }
protected:
    void closeEvent(QCloseEvent *) override;
private:
    void tick();
    void cameraOn(bool on);
    void stopCamera();
    Core *core_;
    QLabel *name_, *status_;
    CallVideo *video_;
    QPushButton *answer_, *mute_, *camera_, *hangup_;
    QCamera *cam_ = nullptr;
    QMediaCaptureSession *session_ = nullptr;
    QVideoSink *sink_ = nullptr;
    QElapsedTimer sent_;
    bool hasVideo_ = false, remoteShows_ = true, gotRemote_ = false;
    QTimer *timer_;
    QElapsedTimer since_;
    QString state_;
};

/* The picture of a message in a window of its own, with Save as, Copy and zoom. */
class ImageViewer : public QDialog {
    Q_OBJECT
public:
    ImageViewer(const QString &path, const QString &name, QWidget *parent);
signals:
    void saveRequested();
protected:
    void resizeEvent(QResizeEvent *e) override;
    void keyPressEvent(QKeyEvent *e) override;
    bool eventFilter(QObject *obj, QEvent *ev) override;
private:
    void zoomBy(double factor);
    void render();
    QImage image_;
    QLabel *label_, *info_;
    QScrollArea *area_;
    double zoom_ = 0; /* 0 = fit the window, otherwise the scale (1 = actual size) */
};

/* The contents of a text file in a window of its own (read-only, monospace): Save as, Copy, word wrap and find. */
class TextViewer : public QDialog {
    Q_OBJECT
public:
    TextViewer(const QString &name, const QString &text, qint64 size, bool truncated, QWidget *parent);
signals:
    void saveRequested();
};

/* Name and topic of the open room */
class RoomSettingsDialog : public QDialog {
    Q_OBJECT
public:
    RoomSettingsDialog(Core *core, const QJsonObject &details, QWidget *parent);
};

void showAbout(QWidget *parent);

}

#endif
