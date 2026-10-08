#ifndef VC_QT_AUDIOPLAYER_H
#define VC_QT_AUDIOPLAYER_H

#include <QAudioDecoder>
#include <QAudioOutput>
#include <QDialog>
#include <QMediaPlayer>
#include <QVector>

class QLabel;
class QSlider;
class QToolButton;

namespace vc {

/* A waveform like SoundCloud's: mirrored bars from the loudness of the whole file; the part already played is drawn in the accent colour.
   A click or drag seeks. */
class WaveformView : public QWidget {
    Q_OBJECT
public:
    explicit WaveformView(QWidget *parent = nullptr);
    void setPeaks(const QVector<float> &peaks, bool final);
    void setProgress(double fraction) { progress_ = fraction; update(); }
    QSize sizeHint() const override { return QSize(520, 120); }
signals:
    void seekRequested(double fraction);

protected:
    void paintEvent(QPaintEvent *) override;
    void mousePressEvent(QMouseEvent *e) override;
    void mouseMoveEvent(QMouseEvent *e) override;

private:
    QVector<float> peaks_; /* 0..1, one per slice of the file */
    bool final_ = false;
    double progress_ = 0;
};

/* A window that plays an audio file with its waveform. */
class AudioPlayer : public QDialog {
    Q_OBJECT
public:
    AudioPlayer(const QString &path, const QString &title, QWidget *parent = nullptr);

private:
    void toggle();
    void refresh();
    void onBuffer();

    QMediaPlayer *player_;
    QAudioOutput *audio_;
    QAudioDecoder *decoder_;
    WaveformView *wave_;
    QToolButton *btn_;
    QLabel *time_, *status_;
    QSlider *volume_;
    QVector<float> peaks_;
};

} // namespace vc

#endif
