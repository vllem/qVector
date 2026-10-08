#include "audioplayer.h"

#include <QAudioBuffer>
#include <QHBoxLayout>
#include <QLabel>
#include <QMouseEvent>
#include <QPainter>
#include <QSettings>
#include <QSlider>
#include <QStyle>
#include <QTimer>
#include <QToolButton>
#include <QUrl>
#include <QVBoxLayout>
#include <cmath>
#include <complex>

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
    const bool dark = palette().color(QPalette::Window).lightness() < 128;
    const QColor playedTop("#ff5500"), playedLow("#ff9a66");
    const QColor restTop = dark ? QColor("#b0b0b0") : QColor("#333333"), restLow = dark ? QColor("#5c5c5c") : QColor("#b8b8b8");
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

void SpectrumView::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    const int n = levels_.size();
    if (n == 0) return;
    const double w = double(width()) / n;
    for (int i = 0; i < n; i++) {
        const double h = levels_[i] > 0 ? qMax(1.0, levels_[i] * (height() - 2)) : 0.0;
        QColor c = QColor::fromHsvF(0.04 + 0.08 * levels_[i], 0.95, 1.0); /* orange, warmer when louder */
        p.fillRect(QRectF(i * w + 1, height() - h, qMax(1.0, w - 2), h), c);
    }
}

/* In-place radix-2 FFT of a power-of-two sized buffer. */
static void fft(QVector<std::complex<float>> &a)
{
    const int n = a.size();
    for (int i = 1, j = 0; i < n; i++) {
        int bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap(a[i], a[j]);
    }
    for (int len = 2; len <= n; len <<= 1) {
        const std::complex<float> wl(std::cos(2 * float(M_PI) / len), -std::sin(2 * float(M_PI) / len));
        for (int i = 0; i < n; i += len) {
            std::complex<float> w(1);
            for (int k = 0; k < len / 2; k++) {
                const auto u = a[i + k], v = a[i + k + len / 2] * w;
                a[i + k] = u + v; a[i + k + len / 2] = u - v;
                w *= wl;
            }
        }
    }
}

AudioPlayer::AudioPlayer(const QString &path, const QString &title, QWidget *parent) : QDialog(parent)
{
    setWindowTitle(title.isEmpty() ? "Audio" : title);
    setAttribute(Qt::WA_DeleteOnClose);
    auto *v = new QVBoxLayout(this);
    auto *name = new QLabel(title, this);
    QFont f = name->font(); f.setBold(true); name->setFont(f);
    wave_ = new WaveformView(this);
    wave_->setMinimumHeight(110);
    spectrum_ = new SpectrumView(this);
    status_ = new QLabel(this);
    status_->setEnabled(false);
    v->addWidget(name);
    v->addWidget(wave_, 1);
    v->addWidget(spectrum_);
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
    spectrumTimer_ = new QTimer(this);
    spectrumTimer_->setInterval(33);
    connect(spectrumTimer_, &QTimer::timeout, this, &AudioPlayer::tickSpectrum);
    resize(560, 340);
    refresh();
    player_->play(); /* clicked to be heard */
}

void AudioPlayer::onBuffer()
{
    const QAudioBuffer b = decoder_->read();
    if (!b.isValid()) return;
    const QAudioFormat fmt = b.format();
    const int ch = qMax(1, fmt.channelCount()), frames = b.frameCount(), slice = 1024;
    rate_ = fmt.sampleRate() > 0 ? fmt.sampleRate() : 44100;
    if (mono_.size() < rate_ * 60 * 30) { /* keep up to half an hour for the analyser */
        auto keep = [&](auto *d, double scale, int off) {
            for (int f = 0; f < frames; f++) { double sum = 0; for (int c = 0; c < ch; c++) sum += double(d[f * ch + c]) - off; mono_.append(qint16(qBound(-1.0, sum / ch / scale, 1.0) * 32767)); }
        };
        switch (fmt.sampleFormat()) {
        case QAudioFormat::UInt8: keep(b.constData<quint8>(), 128.0, 128); break;
        case QAudioFormat::Int16: keep(b.constData<qint16>(), 32768.0, 0); break;
        case QAudioFormat::Int32: keep(b.constData<qint32>(), 2147483648.0, 0); break;
        case QAudioFormat::Float: keep(b.constData<float>(), 1.0, 0); break;
        default: break;
        }
    }
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

/* The bars: a Hann-windowed FFT of the 2048 samples at the playing position, grouped on a log frequency scale, in decibels, with a quick rise
   and a slow fall. */
void AudioPlayer::tickSpectrum()
{
    const int N = 2048, nBars = 48;
    if (bars_.size() != nBars) bars_ = QVector<float>(nBars, 0.f);
    const bool playing = player_->playbackState() == QMediaPlayer::PlayingState;
    const qint64 start = qint64(player_->position()) * rate_ / 1000 - N / 2;
    QVector<float> target(nBars, 0.f);
    if (playing && !mono_.isEmpty()) {
        QVector<std::complex<float>> a(N);
        for (int i = 0; i < N; i++) {
            const qint64 at = start + i;
            const float x = at >= 0 && at < mono_.size() ? mono_[int(at)] / 32768.f : 0.f;
            a[i] = x * (0.5f - 0.5f * std::cos(2 * float(M_PI) * i / (N - 1)));
        }
        fft(a);
        const double lo = 40.0, hi = qMin(16000.0, rate_ / 2.0 - 1);
        for (int b = 0; b < nBars; b++) {
            const double f0 = lo * std::pow(hi / lo, double(b) / nBars), f1 = lo * std::pow(hi / lo, double(b + 1) / nBars);
            const int k0 = qBound(1, int(f0 * N / rate_), N / 2 - 1), k1 = qBound(k0 + 1, int(std::ceil(f1 * N / rate_)), N / 2);
            float m = 0;
            for (int k = k0; k < k1; k++) m = qMax(m, std::abs(a[k]));
            const float db = 20.f * std::log10(qMax(m / (N / 4.f), 1e-5f)); /* 0 dB is a full-scale tone */
            target[b] = qBound(0.f, (db + 70.f) / 70.f, 1.f);
        }
    }
    bool any = false;
    for (int i = 0; i < nBars; i++) {
        bars_[i] = target[i] > bars_[i] ? bars_[i] + (target[i] - bars_[i]) * 0.6f : bars_[i] * 0.88f;
        if (bars_[i] > 0.004f) any = true; else bars_[i] = 0;
    }
    spectrum_->setLevels(bars_);
    if (!playing && !any) spectrumTimer_->stop();
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
    if (playing && !spectrumTimer_->isActive()) spectrumTimer_->start();
    btn_->setIcon(style()->standardIcon(playing ? QStyle::SP_MediaPause : ended ? QStyle::SP_BrowserReload : QStyle::SP_MediaPlay));
    btn_->setToolTip(playing ? "Pause" : ended ? "Play again" : "Play");
}

} // namespace vc
