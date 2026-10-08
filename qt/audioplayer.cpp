#include "audioplayer.h"

#include "qt/theme.h"
#include <QAudioBuffer>
#include <QHBoxLayout>
#include <QLabel>
#include <QMouseEvent>
#include <QPainter>
#include <QSettings>
#include <QSlider>
#include <QStyle>
#include <QToolButton>
#include <QUrl>
#include <QVBoxLayout>
#include <cmath>

namespace vc {

static QString clock(qint64 ms)
{
    const int s = int(ms / 1000);
    return QString("%1:%2").arg(s / 60).arg(s % 60, 2, 10, QLatin1Char('0'));
}

WaveformView::WaveformView(QWidget *parent) : QWidget(parent)
{
    setMinimumHeight(90);
    setCursor(Qt::PointingHandCursor);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
}

void WaveformView::setPeaks(const QVector<float> &peaks, bool final)
{
    peaks_ = peaks;
    final_ = final;
    update();
}

/* SoundCloud's look: thin bars (2 px, 1 px apart), most of the height above a baseline and a shorter, fainter reflection below it; the played part
   is orange, the rest grey. */
void WaveformView::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, false);
    const QPalette &pal = palette();
    const QColor playedTop = pal.color(QPalette::Highlight), playedLow = blend(playedTop, pal.color(QPalette::Window), 0.45);
    const QColor restTop = blend(pal.color(QPalette::WindowText), pal.color(QPalette::Window), 0.2), restLow = blend(pal.color(QPalette::WindowText), pal.color(QPalette::Window), 0.65);
    const int barW = 2, gap = 1, step = barW + gap;
    const int n = qMax(1, width() / step);
    const double base = height() * 0.70; /* the baseline */
    const double up = base - 2, down = (height() - base) - 1;
    const int px = int(progress_ * width());
    for (int i = 0; i < n; i++) {
        float v = 0.03f; /* a thin line while the file is still being read */
        if (!peaks_.isEmpty()) {
            const int a = int(double(i) * peaks_.size() / n), b = qMax(a + 1, int(double(i + 1) * peaks_.size() / n));
            v = 0;
            for (int k = a; k < b && k < peaks_.size(); k++) v = qMax(v, peaks_[k]);
            v = qMax(0.03f, v);
        }
        const int x = i * step;
        const bool done = x + barW <= px;
        const double hu = qMax(1.0, v * up), hl = qMax(1.0, v * down * 0.75);
        p.fillRect(QRectF(x, base - hu, barW, hu), done ? playedTop : restTop);
        p.fillRect(QRectF(x, base + 1, barW, hl), done ? playedLow : restLow);
    }
    if (!final_) { p.setPen(palette().color(QPalette::PlaceholderText)); p.drawText(rect().adjusted(0, 0, -6, -2), Qt::AlignRight | Qt::AlignBottom, "analysing..."); }
}

void WaveformView::mousePressEvent(QMouseEvent *e) { if (e->button() == Qt::LeftButton) emit seekRequested(qBound(0.0, e->position().x() / width(), 1.0)); }
void WaveformView::mouseMoveEvent(QMouseEvent *e) { if (e->buttons() & Qt::LeftButton) emit seekRequested(qBound(0.0, e->position().x() / width(), 1.0)); }

AudioPlayer::AudioPlayer(const QString &path, const QString &title, QWidget *parent) : QDialog(parent)
{
    setWindowTitle(title.isEmpty() ? "Audio" : title);
    setAttribute(Qt::WA_DeleteOnClose);
    auto *v = new QVBoxLayout(this);
    auto *name = new QLabel(title, this);
    QFont f = name->font(); f.setBold(true); name->setFont(f);
    wave_ = new WaveformView(this);
    wave_->setMinimumHeight(110);
    status_ = new QLabel(this);
    status_->setEnabled(false);
    v->addWidget(name);
    v->addWidget(wave_, 1);
    auto *row = new QHBoxLayout;
    btn_ = new QToolButton(this);
    btn_->setIconSize(QSize(24, 24));
    time_ = new QLabel("0:00 / 0:00", this);
    volume_ = new QSlider(Qt::Horizontal, this);
    volume_->setRange(0, 100);
    volume_->setFixedWidth(90);
    volume_->setToolTip("Volume");
    row->addWidget(btn_);
    row->addWidget(time_);
    row->addWidget(status_, 1);
    row->addWidget(volume_);
    v->addLayout(row);

    player_ = new QMediaPlayer(this);
    audio_ = new QAudioOutput(this);
    player_->setAudioOutput(audio_);
    QSettings st("vector", "vector");
    volume_->setValue(st.value("video_volume", 80).toInt());
    audio_->setVolume(volume_->value() / 100.0f);
    connect(volume_, &QSlider::valueChanged, this, [this](int val) { audio_->setVolume(val / 100.0f); QSettings("vector", "vector").setValue("video_volume", val); });
    connect(btn_, &QToolButton::clicked, this, &AudioPlayer::toggle);
    connect(wave_, &WaveformView::seekRequested, this, [this](double f) { if (player_->duration() > 0) player_->setPosition(qint64(f * player_->duration())); });
    connect(player_, &QMediaPlayer::positionChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::durationChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::playbackStateChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::mediaStatusChanged, this, [this] { refresh(); });
    connect(player_, &QMediaPlayer::errorOccurred, this, [this](QMediaPlayer::Error, const QString &msg) { status_->setText("Cannot play: " + msg); });
    player_->setSource(QUrl::fromLocalFile(path));

    /* the waveform: decode the file once into loudness slices of about 1024 frames */
    decoder_ = new QAudioDecoder(this);
    connect(decoder_, &QAudioDecoder::bufferReady, this, &AudioPlayer::onBuffer);
    connect(decoder_, &QAudioDecoder::finished, this, [this] {
        float top = 0; for (float x : peaks_) top = qMax(top, x);
        if (top > 0) for (float &x : peaks_) x = std::pow(x / top, 0.8f); /* the loudest bar reaches the top; quiet parts stay visible */
        wave_->setPeaks(peaks_, true);
    });
    connect(decoder_, qOverload<QAudioDecoder::Error>(&QAudioDecoder::error), this, [this](QAudioDecoder::Error) { wave_->setPeaks(peaks_, true); });
    decoder_->setSource(QUrl::fromLocalFile(path));
    decoder_->start();
    resize(560, 230);
    refresh();
    player_->play(); /* clicked to be heard */
}

void AudioPlayer::onBuffer()
{
    const QAudioBuffer b = decoder_->read();
    if (!b.isValid()) return;
    const QAudioFormat fmt = b.format();
    const int ch = qMax(1, fmt.channelCount()), frames = b.frameCount(), slice = 1024;
    auto scan = [&](auto *data, double scale) {
        for (int f0 = 0; f0 < frames; f0 += slice) {
            float peak = 0;
            const int end = qMin(frames, f0 + slice);
            for (int i = f0 * ch; i < end * ch; i++) peak = qMax(peak, float(std::abs(double(data[i])) / scale));
            peaks_.append(qMin(1.0f, peak));
        }
    };
    switch (fmt.sampleFormat()) {
    case QAudioFormat::UInt8: { const quint8 *d = b.constData<quint8>(); QVector<qint16> t(frames * ch); for (int i = 0; i < t.size(); i++) t[i] = qint16(int(d[i]) - 128); scan(t.constData(), 128.0); break; }
    case QAudioFormat::Int16: scan(b.constData<qint16>(), 32768.0); break;
    case QAudioFormat::Int32: scan(b.constData<qint32>(), 2147483648.0); break;
    case QAudioFormat::Float: scan(b.constData<float>(), 1.0); break;
    default: break;
    }
    if (peaks_.size() % 64 == 0) wave_->setPeaks(peaks_, false); /* grows while decoding */
}

void AudioPlayer::toggle()
{
    if (player_->playbackState() == QMediaPlayer::PlayingState) player_->pause();
    else { if (player_->mediaStatus() == QMediaPlayer::EndOfMedia) player_->setPosition(0); player_->play(); }
}

void AudioPlayer::refresh()
{
    const qint64 d = player_->duration(), p = player_->position();
    time_->setText(clock(p) + " / " + clock(d));
    wave_->setProgress(d > 0 ? double(p) / d : 0);
    const bool playing = player_->playbackState() == QMediaPlayer::PlayingState, ended = player_->mediaStatus() == QMediaPlayer::EndOfMedia;
    btn_->setIcon(style()->standardIcon(playing ? QStyle::SP_MediaPause : ended ? QStyle::SP_BrowserReload : QStyle::SP_MediaPlay));
    btn_->setToolTip(playing ? "Pause" : ended ? "Play again" : "Play");
}

} // namespace vc
