#ifndef VC_QT_VIDEOPLAYER_H
#define VC_QT_VIDEOPLAYER_H

#include <QAudioOutput>
#include <QLabel>
#include <QMediaPlayer>
#include <QSlider>
#include <QToolButton>
#include <QVideoWidget>
#include <QWidget>

namespace vc {

/* A slider that jumps to where it is clicked (QSlider only pages) and reports the position the reader asked for. */
class SeekSlider : public QSlider {
    Q_OBJECT
public:
    explicit SeekSlider(QWidget *parent = nullptr) : QSlider(Qt::Horizontal, parent) {}
signals:
    void seekRequested(int value); /* while dragging and on a click */

protected:
    void mousePressEvent(QMouseEvent *e) override;
    void mouseMoveEvent(QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;

private:
    int valueAt(const QPoint &p) const;
    bool dragging_ = false;
};

/* Qt Multimedia video with play/pause, seek bar, time and volume. The control row has a fixed height (kControlsHeight): the timeline reserves exactly
   that much below the picture, so the controls sit inside the card. Click the picture or press Space to play/pause, Left/Right to skip 5 s, M to mute. */
class VideoPlayer : public QWidget {
    Q_OBJECT
public:
    static constexpr int kControlsHeight = 34;
    explicit VideoPlayer(QWidget *parent = nullptr);
    ~VideoPlayer() override;
    bool open(const QString &path);
    bool openUrl(const QUrl &url); /* a network stream */
    QString error() const { return error_; }

signals:
    void closeRequested();

protected:
    void keyPressEvent(QKeyEvent *e) override;
    bool eventFilter(QObject *obj, QEvent *ev) override;

private:
    void refresh();
    void togglePlay();
    void setMuted(bool on);
    QMediaPlayer *player_;
    QAudioOutput *audio_;
    QVideoWidget *video_;
    QToolButton *btn_, *close_, *mute_;
    SeekSlider *slider_;
    QSlider *volume_;
    QLabel *time_, *errLabel_;
    QString error_;
    bool seeking_ = false;
};

}

#endif
