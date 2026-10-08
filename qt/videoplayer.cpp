#include "qt/theme.h"
#include "qt/videoplayer.h"
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QSettings>
#include <QStyle>
#include <QStyleOptionSlider>
#include <QUrl>
#include <QVBoxLayout>
#include <cstdio>

namespace vc {

static QString fmt(qint64 ms)
{
    qint64 s = ms / 1000;
    if (s >= 3600) return QString("%1:%2:%3").arg(s / 3600).arg(s / 60 % 60, 2, 10, QChar('0')).arg(s % 60, 2, 10, QChar('0'));
    return QString("%1:%2").arg(s / 60).arg(s % 60, 2, 10, QChar('0'));
}

/* ---- the seek bar ---- */

int SeekSlider::valueAt(const QPoint &p) const
{
    QStyleOptionSlider opt;
    initStyleOption(&opt);
    const QRect groove = style()->subControlRect(QStyle::CC_Slider, &opt, QStyle::SC_SliderGroove, this);
    const QRect handle = style()->subControlRect(QStyle::CC_Slider, &opt, QStyle::SC_SliderHandle, this);
    const int span = groove.width() - handle.width();
    return QStyle::sliderValueFromPosition(minimum(), maximum(), p.x() - groove.x() - handle.width() / 2, qMax(1, span));
}

void SeekSlider::mousePressEvent(QMouseEvent *e)
{
    if (e->button() != Qt::LeftButton) { QSlider::mousePressEvent(e); return; }
    dragging_ = true;
    setSliderDown(true);
    setValue(valueAt(e->position().toPoint())); /* a click on the bar goes there at once */
    emit seekRequested(value());
    e->accept();
}

void SeekSlider::mouseMoveEvent(QMouseEvent *e)
{
    if (!dragging_) { QSlider::mouseMoveEvent(e); return; }
    setValue(valueAt(e->position().toPoint()));
    emit seekRequested(value()); /* the picture follows while dragging */
    e->accept();
}

void SeekSlider::mouseReleaseEvent(QMouseEvent *e)
{
    if (!dragging_) { QSlider::mouseReleaseEvent(e); return; }
    dragging_ = false;
    setSliderDown(false);
    e->accept();
}

/* ---- the player ---- */

VideoPlayer::VideoPlayer(QWidget *parent) : QWidget(parent)
{
    setFocusPolicy(Qt::StrongFocus);
    setAutoFillBackground(true);
    QPalette dark = palette();
    dark.setColor(QPalette::Window, Qt::black);
    setPalette(dark);
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(0, 0, 0, 0);
    v->setSpacing(0);
    video_ = new QVideoWidget(this);
    video_->setMinimumSize(40, 30);
    video_->installEventFilter(this);
    setMinimumSize(0, 0);
    v->addWidget(video_, 1);
    errLabel_ = new QLabel(this);
    errLabel_->setStyleSheet("color:" + errorColor(palette()).name() + ";padding:4px 8px");
    errLabel_->setWordWrap(true);
    errLabel_->hide();
    v->addWidget(errLabel_);

    auto *controls = new QWidget(this);
    controls->setFixedHeight(kControlsHeight);
    controls->setStyleSheet("QWidget { background: palette(window); color: palette(window-text); } QLabel { background: transparent; }"
                            "QToolButton { background: transparent; border: none; border-radius: 3px; padding: 2px; }"
                            "QToolButton:hover { background: palette(midlight); }"
                            "QSlider::groove:horizontal { height: 4px; background: palette(mid); border-radius: 2px; }"
                            "QSlider::sub-page:horizontal { background: palette(highlight); border-radius: 2px; }"
                            "QSlider::handle:horizontal { width: 10px; margin: -5px 0; background: palette(window-text); border-radius: 5px; }");
    auto *row = new QHBoxLayout(controls);
    row->setContentsMargins(8, 2, 8, 2);
    row->setSpacing(6);
    btn_ = new QToolButton(controls);
    btn_->setIconSize(QSize(16, 16));
    slider_ = new SeekSlider(controls);
    slider_->setRange(0, 1000);
    time_ = new QLabel("0:00 / 0:00", controls);
    time_->setMinimumWidth(time_->fontMetrics().horizontalAdvance("00:00:00 / 00:00:00") * 3 / 4);
    mute_ = new QToolButton(controls);
    mute_->setIconSize(QSize(16, 16));
    volume_ = new QSlider(Qt::Horizontal, controls);
    volume_->setRange(0, 100);
    volume_->setFixedWidth(60);
    close_ = new QToolButton(controls);
    close_->setText(QString(QChar(0x00D7))); /* a plain multiplication sign: a symbol font could draw a coloured emoji for the cross */
    close_->setStyleSheet("QToolButton { font-size: 16pt; padding: 0 6px; }");
    close_->setToolTip("Close the player");
    row->addWidget(btn_);
    row->addWidget(slider_, 1);
    row->addWidget(time_);
    row->addWidget(mute_);
    row->addWidget(volume_);
    row->addWidget(close_);
    v->addWidget(controls);
    connect(close_, &QToolButton::clicked, this, &VideoPlayer::closeRequested);

    player_ = new QMediaPlayer(this);
    audio_ = new QAudioOutput(this);
    player_->setAudioOutput(audio_);
    player_->setVideoOutput(video_);
    {
        QSettings st("vector", "vector");
        volume_->setValue(st.value("video_volume", 80).toInt());
        audio_->setVolume(volume_->value() / 100.0f);
        audio_->setMuted(st.value("video_muted", false).toBool());
    }
    connect(volume_, &QSlider::valueChanged, this, [this](int v2) {
        audio_->setVolume(v2 / 100.0f);
        if (v2 > 0 && audio_->isMuted()) setMuted(false);
        QSettings("vector", "vector").setValue("video_volume", v2);
        refresh();
    });
    connect(mute_, &QToolButton::clicked, this, [this] { setMuted(!audio_->isMuted()); });
    connect(btn_, &QToolButton::clicked, this, &VideoPlayer::togglePlay);
    connect(slider_, &SeekSlider::seekRequested, this, [this](int value) {
        seeking_ = true;
        if (player_->duration() > 0) player_->setPosition(player_->duration() * value / 1000);
    });
    connect(slider_, &QSlider::sliderReleased, this, [this] { seeking_ = false; });
    connect(player_, &QMediaPlayer::positionChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::durationChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::playbackStateChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::mediaStatusChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::errorOccurred, this, [this](QMediaPlayer::Error, const QString &msg) {
        error_ = msg;
        fprintf(stderr, "video: error: %s\n", msg.toUtf8().constData());
        errLabel_->setText("Cannot play the video: " + msg);
        errLabel_->show();
    });
    refresh();
}

VideoPlayer::~VideoPlayer()
{
    player_->stop();
    player_->setVideoOutput(static_cast<QVideoWidget *>(nullptr));
}

void VideoPlayer::togglePlay()
{
    if (player_->playbackState() == QMediaPlayer::PlayingState) player_->pause();
    else {
        if (player_->mediaStatus() == QMediaPlayer::EndOfMedia) player_->setPosition(0);
        player_->play();
    }
}

void VideoPlayer::setMuted(bool on)
{
    audio_->setMuted(on);
    QSettings("vector", "vector").setValue("video_muted", on);
    refresh();
}

void VideoPlayer::keyPressEvent(QKeyEvent *e)
{
    switch (e->key()) {
    case Qt::Key_Space: case Qt::Key_K: togglePlay(); break;
    case Qt::Key_Left: player_->setPosition(qMax<qint64>(0, player_->position() - 5000)); break;
    case Qt::Key_Right: player_->setPosition(qMin(player_->duration(), player_->position() + 5000)); break;
    case Qt::Key_M: setMuted(!audio_->isMuted()); break;
    case Qt::Key_Escape: emit closeRequested(); break;
    default: QWidget::keyPressEvent(e); return;
    }
    e->accept();
}

/* a click on the picture plays / pauses */
bool VideoPlayer::eventFilter(QObject *obj, QEvent *ev)
{
    if (obj == video_ && ev->type() == QEvent::MouseButtonRelease && static_cast<QMouseEvent *>(ev)->button() == Qt::LeftButton) {
        setFocus();
        togglePlay();
        return true;
    }
    return QWidget::eventFilter(obj, ev);
}

bool VideoPlayer::open(const QString &path)
{
    fprintf(stderr, "video: playing %s with Qt Multimedia\n", path.toUtf8().constData());
    player_->setSource(QUrl::fromLocalFile(path));
    player_->play();
    return true; /* failures arrive asynchronously through errorOccurred */
}

bool VideoPlayer::openUrl(const QUrl &url)
{
    fprintf(stderr, "video: playing a stream from %s with Qt Multimedia\n", url.host().toUtf8().constData());
    player_->setSource(url);
    player_->play();
    return true;
}

void VideoPlayer::refresh()
{
    const qint64 d = player_->duration(), p = player_->position();
    const auto st = player_->playbackState();
    const bool ended = player_->mediaStatus() == QMediaPlayer::EndOfMedia;
    btn_->setIcon(style()->standardIcon(st == QMediaPlayer::PlayingState ? QStyle::SP_MediaPause : ended ? QStyle::SP_BrowserReload : QStyle::SP_MediaPlay));
    btn_->setToolTip(st == QMediaPlayer::PlayingState ? "Pause" : ended ? "Play again" : "Play");
    mute_->setIcon(style()->standardIcon(audio_->isMuted() || volume_->value() == 0 ? QStyle::SP_MediaVolumeMuted : QStyle::SP_MediaVolume));
    mute_->setToolTip(audio_->isMuted() ? "Unmute" : "Mute");
    time_->setText(fmt(p) + " / " + (d > 0 ? fmt(d) : QString("--:--")));
    if (!seeking_ && d > 0) { QSignalBlocker b(slider_); slider_->setValue(int(p * 1000 / d)); }
}

}
