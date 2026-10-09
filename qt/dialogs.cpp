#include "qt/dialogs.h"
#include "qt/theme.h"
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QCloseEvent>
#include <QDateTime>
#include <QFileInfo>
#include <QFontDatabase>
#include <QFormLayout>
#include <QImageReader>
#include <QCamera>
#include <QKeyEvent>
#include <QMediaCaptureSession>
#include <QMediaDevices>
#include <QAudioDevice>
#include <QSoundEffect>
#if __has_include(<QScreenCapture>)
#include <QScreenCapture>
#define VC_HAVE_SCREENCAPTURE 1
#endif
#include <QGuiApplication>
#include <QScreen>
#include <QStandardPaths>
#include <QDir>
#include <QFile>
#include <QtMath>
#include <QPainter>
#include <QVideoFrame>
#include <QVideoSink>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QShortcut>
#include <QToolBar>
#include <QWheelEvent>
#include <functional>

namespace vc {

static QString S(const QJsonObject &o, const char *k) { return o.value(QLatin1String(k)).toString(); }

/* ---------- saved messages ---------- */

SavedDialog::SavedDialog(Core *core, QWidget *parent) : QDialog(parent), core_(core)
{
    setWindowTitle("Saved messages");
    resize(620, 440);
    auto *v = new QVBoxLayout(this);
    list_ = new QListWidget;
    list_->setWordWrap(true);
    list_->setAlternatingRowColors(true);
    v->addWidget(list_, 1);
    auto *row = new QHBoxLayout;
    auto *show = new QPushButton("Show in room");
    auto *remove = new QPushButton("Remove");
    auto *close = new QPushButton("Close");
    row->addWidget(show);
    row->addWidget(remove);
    row->addStretch(1);
    row->addWidget(close);
    v->addLayout(row);
    auto go = [this] {
        if (QListWidgetItem *it = list_->currentItem()) {
            emit showRequested(it->data(Qt::UserRole).toString(), it->data(Qt::UserRole + 1).toString());
            this->close();
        }
    };
    connect(show, &QPushButton::clicked, this, go);
    connect(list_, &QListWidget::itemActivated, this, go);
    connect(remove, &QPushButton::clicked, this, [this] { if (QListWidgetItem *it = list_->currentItem()) core_->call("remove_bookmark", {{"event_id", it->data(Qt::UserRole + 1).toString()}}); });
    connect(close, &QPushButton::clicked, this, &QDialog::close);
}

void SavedDialog::setBookmarks(const QJsonArray &list)
{
    const QString keep = list_->currentItem() ? list_->currentItem()->data(Qt::UserRole + 1).toString() : QString();
    list_->clear();
    for (const QJsonValue &v : list) {
        const QJsonObject b = v.toObject();
        auto *it = new QListWidgetItem(QString("%1  ·  %2\n%3").arg(S(b, "sender"), S(b, "time"), S(b, "body")));
        it->setData(Qt::UserRole, S(b, "room_id"));
        it->setData(Qt::UserRole + 1, S(b, "event_id"));
        list_->addItem(it);
        if (S(b, "event_id") == keep) list_->setCurrentItem(it);
    }
    if (list.isEmpty()) {
        auto *it = new QListWidgetItem("Nothing saved yet. Right-click a message and choose \"Save message\".");
        it->setFlags(Qt::NoItemFlags);
        list_->addItem(it);
    }
}

/* ---------- polls ---------- */

PollDialog::PollDialog(Core *core, QWidget *parent) : QDialog(parent)
{
    setWindowTitle("Create poll");
    setMinimumWidth(460);
    auto *v = new QVBoxLayout(this);
    auto *question = new QLineEdit;
    question->setPlaceholderText("Question");
    v->addWidget(question);
    auto *answers = new QPlainTextEdit;
    answers->setPlaceholderText("One answer per line (2 to 20)");
    answers->setMinimumHeight(110);
    v->addWidget(answers);
    auto *several = new QCheckBox("Allow choosing several answers");
    v->addWidget(several);
    auto *error = new QLabel;
    error->setStyleSheet("color:" + errorColor(qApp->palette()).name());
    v->addWidget(error);
    auto *row = new QHBoxLayout;
    auto *send = new QPushButton("Send poll");
    auto *cancel = new QPushButton("Cancel");
    row->addWidget(send);
    row->addWidget(cancel);
    row->addStretch(1);
    v->addLayout(row);
    connect(cancel, &QPushButton::clicked, this, &QDialog::reject);
    connect(send, &QPushButton::clicked, this, [=] {
        QJsonArray options;
        for (const QString &l : answers->toPlainText().split('\n')) if (!l.trimmed().isEmpty()) options.append(l.trimmed());
        if (question->text().trimmed().isEmpty() || options.size() < 2 || options.size() > 20) {
            error->setText("A poll needs a question and 2 to 20 answers.");
            return;
        }
        core->call("create_poll", {{"question", question->text().trimmed()}, {"options", options}, {"multiple", several->isChecked()}});
        accept();
    });
}

/* ---------- recovery key ---------- */

RecoveryDialog::RecoveryDialog(Core *core, bool create, QWidget *parent) : QDialog(parent), core_(core), create_(create)
{
    setWindowTitle(create ? "Set up recovery" : "Enter recovery key");
    setMinimumWidth(540);
    auto *v = new QVBoxLayout(this);
    info_ = new QLabel(create ? "Creates a recovery key for this account (and a backup of your message keys). With it, any new session can read your old encrypted messages. "
                                "Your server may ask for your account password."
                              : "Messages sent before you signed in on this device are encrypted. Enter the recovery key (or passphrase) of this account, from your secure backup "
                                "(Element: Settings > Security & Privacy > Secure Backup), to read them and to show other people that this session is yours.");
    info_->setWordWrap(true);
    v->addWidget(info_);
    key_ = new QLineEdit;
    key_->setPlaceholderText("Recovery key (EsTc LW2K ...)");
    key_->setVisible(!create);
    v->addWidget(key_);
    password_ = new QLineEdit;
    password_->setEchoMode(QLineEdit::Password);
    password_->setPlaceholderText("Account password");
    password_->setVisible(false);
    v->addWidget(password_);
    shown_ = new QLineEdit;
    shown_->setReadOnly(true);
    shown_->setVisible(false);
    shown_->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    v->addWidget(shown_);
    status_ = new QLabel;
    status_->setWordWrap(true);
    v->addWidget(status_);
    auto *row = new QHBoxLayout;
    go_ = new QPushButton(create ? "Create" : "Unlock");
    copy_ = new QPushButton("Copy recovery key");
    copy_->setVisible(false);
    auto *close = new QPushButton("Close");
    row->addWidget(go_);
    row->addWidget(copy_);
    row->addWidget(close);
    row->addStretch(1);
    v->addLayout(row);
    auto go = [this] {
        status_->setStyleSheet("");
        status_->setText("Working...");
        go_->setEnabled(false);
        if (create_) core_->call("create_recovery", {{"password", password_->text()}});
        else core_->call("recover", {{"key", key_->text()}});
        password_->clear();
    };
    connect(go_, &QPushButton::clicked, this, go);
    connect(key_, &QLineEdit::returnPressed, this, go);
    connect(password_, &QLineEdit::returnPressed, this, go);
    connect(copy_, &QPushButton::clicked, this, [this] { QApplication::clipboard()->setText(shown_->text()); });
    connect(close, &QPushButton::clicked, this, &QDialog::close);
}

void RecoveryDialog::setState(const QJsonObject &s)
{
    const QString st = S(s, "state");
    go_->setEnabled(true);
    if (st == "done") { status_->setStyleSheet("color:" + okColor(qApp->palette()).name()); status_->setText("Unlocked. Your old messages appear as their keys are restored."); go_->setEnabled(false); }
    else if (st == "error") { status_->setStyleSheet("color:" + errorColor(qApp->palette()).name()); status_->setText(S(s, "message")); }
    else if (st == "password") { status_->setStyleSheet(""); status_->setText("Your server wants your account password before it accepts the new keys."); password_->setVisible(true); password_->setFocus(); }
    else if (st == "created") {
        status_->setStyleSheet("");
        status_->setText("This is the only time the recovery key is shown. Write it down or save it in a password manager: it is the way back to your encrypted messages.");
        shown_->setText(S(s, "key")); shown_->setVisible(true); copy_->setVisible(true); password_->setVisible(false); go_->setEnabled(false);
    }
}

/* ---------- verification ---------- */

VerifyDialog::VerifyDialog(Core *core, QWidget *parent) : QDialog(parent), core_(core)
{
    setWindowTitle("Verification");
    setMinimumWidth(600);
    lay_ = new QVBoxLayout(this);
    lay_->setContentsMargins(20, 18, 20, 18);
    lay_->setSpacing(12);
    me_ = core->call("account_info").toObject()["user_id"].toString();
}

void VerifyDialog::closeEvent(QCloseEvent *ev)
{
    if (!finished_) core_->call("cancel_verification"); /* closing the window abandons an unfinished attempt */
    core_->call("dismiss_verification");
    ev->accept();
}

/* A telephone-like tone as a WAV file in the cache: incoming = a double ring (440 + 480 Hz, 0.4 s twice, a pause), outgoing = the ringback (one long 1 s tone, a pause) */
static QString ringFile(bool incoming)
{
    const QString dir = QStandardPaths::writableLocation(QStandardPaths::CacheLocation);
    QDir().mkpath(dir);
    const QString path = dir + (incoming ? "/ring-in.wav" : "/ring-out.wav");
    if (QFile::exists(path)) return path;
    const int rate = 22050;
    const double total = incoming ? 3.0 : 4.0;
    QByteArray pcm;
    QDataStream s(&pcm, QIODevice::WriteOnly);
    s.setByteOrder(QDataStream::LittleEndian);
    for (int i = 0; i < int(total * rate); i++) {
        const double t = double(i) / rate;
        const bool on = incoming ? (t < 0.4 || (t >= 0.6 && t < 1.0)) : t < 1.0;
        double v = on ? 0.25 * (qSin(2 * M_PI * 440 * t) + qSin(2 * M_PI * 480 * t)) : 0.0;
        s << qint16(v * 32767);
    }
    QFile f(path);
    if (!f.open(QIODevice::WriteOnly)) return QString();
    QDataStream w(&f);
    w.setByteOrder(QDataStream::LittleEndian);
    f.write("RIFF"); w << quint32(36 + pcm.size()); f.write("WAVEfmt "); w << quint32(16) << quint16(1) << quint16(1) << quint32(rate) << quint32(rate * 2) << quint16(2) << quint16(16);
    f.write("data"); w << quint32(pcm.size());
    f.write(pcm);
    return path;
}

void CallDialog::ring(const QString &state, bool incoming)
{
    const bool wanted = (state == "incoming" || state == "outgoing") && core_->boolPref("callRing", true);
    if (!wanted) { if (ringer_) ringer_->stop(); return; }
    if (!ringer_) ringer_ = new QSoundEffect(this);
    ringer_->stop();
    const QString file = ringFile(incoming);
    if (file.isEmpty()) return;
    const QString speakers = core_->pref("callSpeakers");
    for (const QAudioDevice &dev : QMediaDevices::audioOutputs())
        if (!speakers.isEmpty() && dev.description() == speakers) ringer_->setAudioDevice(dev);
    ringer_->setSource(QUrl::fromLocalFile(file));
    ringer_->setLoopCount(QSoundEffect::Infinite);
    ringer_->setVolume(0.6f);
    ringer_->play();
}

GroupTiles::GroupTiles(QWidget *parent) : QWidget(parent)
{
    setMinimumSize(420, 280);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
}

void GroupTiles::setPeople(const QJsonArray &people)
{
    people_ = people;
    QStringList keep;
    for (const QJsonValue &v : people) keep << v.toObject()["user_id"].toString();
    for (const QString &k : frames_.keys()) if (!keep.contains(k)) frames_.remove(k);
    update();
}

void GroupTiles::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.fillRect(rect(), QColor(24, 24, 28));
    const int count = people_.size() + 1; /* and ourselves */
    const int cols = qCeil(qSqrt(double(count))), rows = qCeil(double(count) / cols);
    const int gap = 6, tw = (width() - gap * (cols + 1)) / cols, th = (height() - gap * (rows + 1)) / rows;
    p.setRenderHint(QPainter::SmoothPixmapTransform);
    auto tile = [&](int i, const QString &name, const QImage &img, const QString &note) {
        const QRect r(gap + (i % cols) * (tw + gap), gap + (i / cols) * (th + gap), tw, th);
        p.fillRect(r, QColor(40, 40, 46));
        if (!img.isNull()) {
            const QSize s = img.size().scaled(r.size(), Qt::KeepAspectRatio);
            p.drawImage(QRect(r.center() - QPoint(s.width() / 2, s.height() / 2), s), img);
        }
        if (!note.isEmpty()) { p.setPen(QColor(200, 200, 205)); p.drawText(r, Qt::AlignCenter, note); }
        const QRect bar(r.left(), r.bottom() - 22, r.width(), 22);
        p.fillRect(bar, QColor(0, 0, 0, 140));
        p.setPen(Qt::white);
        p.drawText(bar.adjusted(8, 0, -8, 0), Qt::AlignVCenter | Qt::AlignLeft, p.fontMetrics().elidedText(name, Qt::ElideRight, bar.width() - 16));
    };
    int i = 0;
    for (const QJsonValue &v : people_) {
        const QJsonObject o = v.toObject();
        const QString id = o["user_id"].toString();
        const bool shows = o["remote_video"].toBool(true) && frames_.contains(id);
        tile(i++, o["name"].toString().isEmpty() ? id : o["name"].toString(), shows ? frames_.value(id) : QImage(),
             !o["connected"].toBool() ? "Connecting..." : shows ? QString() : "Camera off");
    }
    tile(i, "You", camera_ ? local_ : QImage(), camera_ ? QString() : "Camera off");
}

GroupCallDialog::GroupCallDialog(Core *core, QWidget *parent) : QDialog(parent), core_(core)
{
    setWindowTitle("Group call");
    auto *lay = new QVBoxLayout(this);
    lay->setContentsMargins(20, 18, 20, 16);
    lay->setSpacing(10);
    title_ = new QLabel;
    QFont f = title_->font();
    f.setPointSizeF(f.pointSizeF() * 1.3);
    f.setBold(true);
    title_->setFont(f);
    title_->setAlignment(Qt::AlignCenter);
    lay->addWidget(title_);
    tiles_ = new GroupTiles;
    lay->addWidget(tiles_, 1);
    auto *row = new QHBoxLayout;
    mute_ = new QPushButton("Mute");
    mute_->setCheckable(true);
    camera_ = new QPushButton("Camera");
    camera_->setCheckable(true);
    leave_ = new QPushButton("Leave");
    row->addStretch(1);
    row->addWidget(mute_);
    row->addWidget(camera_);
    row->addWidget(leave_);
    row->addStretch(1);
    lay->addLayout(row);
    connect(mute_, &QPushButton::clicked, this, [this](bool on) { core_->call("set_group_muted", {{"muted", on}}); });
    connect(camera_, &QPushButton::clicked, this, [this](bool on) { cameraOn(on); });
    connect(leave_, &QPushButton::clicked, this, [this] { core_->call("leave_group_call"); });
    connect(core_, &Core::remoteFrame, this, [this](const QString &who, const QImage &img) { if (!who.isEmpty()) tiles_->setFrame(who, img); });
}

GroupCallDialog::~GroupCallDialog() { stopCamera(); }

void GroupCallDialog::useCamera()
{
    if (!camera_->isEnabled() || camera_->isChecked()) return;
    camera_->setChecked(true);
    cameraOn(true);
}

void GroupCallDialog::stopCamera()
{
    if (cam_) cam_->stop();
    delete session_;
    session_ = nullptr;
    delete cam_;
    cam_ = nullptr;
    delete sink_;
    sink_ = nullptr;
    tiles_->setLocal(QImage());
}

void GroupCallDialog::cameraOn(bool on)
{
    if (!on) {
        stopCamera();
        tiles_->setCamera(false);
        core_->call("set_group_camera", {{"on", false}});
        return;
    }
    const auto devices = QMediaDevices::videoInputs();
    if (devices.isEmpty()) {
        camera_->setChecked(false);
        camera_->setEnabled(false);
        camera_->setToolTip("No camera found");
        return;
    }
    QCameraDevice device = devices.first();
    const QString chosen = core_->pref("callCamera");
    for (const QCameraDevice &dev : devices) if (!chosen.isEmpty() && dev.description() == chosen) device = dev;
    cam_ = new QCamera(device, this);
    sink_ = new QVideoSink(this);
    session_ = new QMediaCaptureSession(this);
    session_->setCamera(cam_);
    session_->setVideoSink(sink_);
    sent_.start();
    connect(sink_, &QVideoSink::videoFrameChanged, this, [this](const QVideoFrame &fr) {
        if (!fr.isValid() || sent_.elapsed() < 66) return;
        sent_.restart();
        QImage img = fr.toImage();
        if (img.isNull()) return;
        img = img.convertToFormat(QImage::Format_RGBA8888);
        if (img.width() > 640 || img.height() > 360) img = img.scaled(640, 360, Qt::KeepAspectRatio, Qt::FastTransformation); /* smaller than one-to-one: every connection encodes it */
        core_->pushVideo(img);
        tiles_->setLocal(img.mirrored(true, false));
    });
    connect(cam_, &QCamera::errorOccurred, this, [this](QCamera::Error, const QString &msg) {
        camera_->setChecked(false);
        camera_->setToolTip(msg);
        stopCamera();
        tiles_->setCamera(false);
        core_->call("set_group_camera", {{"on", false}});
    });
    cam_->start();
    tiles_->setCamera(true);
    core_->call("set_group_camera", {{"on", true}});
}

void GroupCallDialog::setState(const QJsonObject &s)
{
    ended_ = s["state"].toString() == "ended";
    const QJsonArray people = s["participants"].toArray();
    tiles_->setPeople(people);
    mute_->setChecked(s["muted"].toBool());
    title_->setText(people.isEmpty() ? "Group call: waiting for others" : QString("Group call with %1 %2").arg(people.size() + 1).arg("people"));
    if (ended_) { stopCamera(); QTimer::singleShot(300, this, &QDialog::close); }
}

void GroupCallDialog::closeEvent(QCloseEvent *ev)
{
    stopCamera();
    if (!ended_) core_->call("leave_group_call");
    ev->accept();
}

CallVideo::CallVideo(QWidget *parent) : QWidget(parent)
{
    setMinimumSize(360, 240);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
}

void CallVideo::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.fillRect(rect(), QColor(24, 24, 28));
    if (!remote_.isNull()) {
        const QSize s = remote_.size().scaled(size(), Qt::KeepAspectRatio);
        p.setRenderHint(QPainter::SmoothPixmapTransform);
        p.drawImage(QRect(QPoint((width() - s.width()) / 2, (height() - s.height()) / 2), s), remote_);
    }
    if (!note_.isEmpty()) {
        p.setPen(QColor(200, 200, 205));
        p.drawText(rect(), Qt::AlignCenter, note_);
    }
    if (!local_.isNull()) {
        const int w = qMin(180, width() / 3);
        const QSize s = local_.size().scaled(w, w, Qt::KeepAspectRatio);
        const QRect r(QPoint(width() - s.width() - 10, height() - s.height() - 10), s);
        p.drawImage(r, local_);
        p.setPen(QPen(QColor(255, 255, 255, 160), 1));
        p.drawRect(r.adjusted(0, 0, -1, -1));
    }
}

CallDialog::CallDialog(Core *core, QWidget *parent) : QDialog(parent), core_(core)
{
    setWindowTitle("Voice call");
    setMinimumWidth(340);
    auto *lay = new QVBoxLayout(this);
    lay->setContentsMargins(24, 22, 24, 20);
    lay->setSpacing(10);
    name_ = new QLabel;
    QFont f = name_->font();
    f.setPointSizeF(f.pointSizeF() * 1.5);
    f.setBold(true);
    name_->setFont(f);
    name_->setAlignment(Qt::AlignCenter);
    status_ = new QLabel;
    status_->setAlignment(Qt::AlignCenter);
    lay->addWidget(name_);
    lay->addWidget(status_);
    video_ = new CallVideo;
    video_->hide();
    lay->addWidget(video_, 1);
    lay->addSpacing(8);
    auto *row = new QHBoxLayout;
    answer_ = new QPushButton("Answer");
    mute_ = new QPushButton("Mute");
    mute_->setCheckable(true);
    camera_ = new QPushButton("Camera");
    camera_->setCheckable(true);
    camera_->hide();
    share_ = new QPushButton("Share screen");
    share_->setCheckable(true);
    share_->hide();
    addVideo_ = new QPushButton("Add video");
    addVideo_->hide();
    hangup_ = new QPushButton("Hang up");
    row->addWidget(answer_);
    row->addWidget(mute_);
    row->addWidget(camera_);
    row->addWidget(share_);
    row->addWidget(addVideo_);
    row->addWidget(hangup_);
    lay->addLayout(row);
    connect(answer_, &QPushButton::clicked, this, [this] { core_->call("answer_call"); answer_->setEnabled(false); });
    connect(mute_, &QPushButton::clicked, this, [this](bool on) { core_->call("set_call_muted", {{"muted", on}}); });
    connect(camera_, &QPushButton::clicked, this, [this](bool on) { cameraOn(on); });
    connect(share_, &QPushButton::clicked, this, [this](bool on) { shareOn(on); });
    connect(addVideo_, &QPushButton::clicked, this, [this] { core_->call("add_call_video"); addVideo_->setEnabled(false); });
    connect(hangup_, &QPushButton::clicked, this, [this] { core_->call("hangup_call"); });
    connect(core_, &Core::remoteFrame, this, [this](const QString &who, const QImage &img) {
        if (!who.isEmpty()) return; /* a group call's picture */
        gotRemote_ = true;
        video_->setRemote(img);
        if (remoteShows_) video_->setNote(QString());
    });
    timer_ = new QTimer(this);
    timer_->setInterval(1000);
    connect(timer_, &QTimer::timeout, this, [this] { tick(); });
}

CallDialog::~CallDialog() { stopCamera(); if (ringer_) ringer_->stop(); }

void CallDialog::stopCamera()
{
    if (cam_) cam_->stop();
#ifdef VC_HAVE_SCREENCAPTURE
    if (screen_) screen_->stop();
#endif
    delete session_; /* the sources and the sink are children of this dialog */
    session_ = nullptr;
    delete cam_;
    cam_ = nullptr;
#ifdef VC_HAVE_SCREENCAPTURE
    delete screen_;
#endif
    screen_ = nullptr;
    delete sink_;
    sink_ = nullptr;
    video_->setLocal(QImage());
}

/* Frames of the camera or of the screen go to the engine, shrunk to what it encodes and limited to about 15 per second. */
void CallDialog::startSource(bool screen)
{
    stopCamera();
    sink_ = new QVideoSink(this);
    session_ = new QMediaCaptureSession(this);
    session_->setVideoSink(sink_);
    sent_.start();
    connect(sink_, &QVideoSink::videoFrameChanged, this, [this, screen](const QVideoFrame &fr) {
        if (!fr.isValid() || sent_.elapsed() < 66) return;
        sent_.restart();
        QImage img = fr.toImage();
        if (img.isNull()) return;
        img = img.convertToFormat(QImage::Format_RGBA8888);
        if (img.width() > 960 || img.height() > 540) img = img.scaled(960, 540, Qt::KeepAspectRatio, Qt::FastTransformation);
        core_->pushVideo(img);
        video_->setLocal(screen ? img : img.mirrored(true, false)); /* the camera as a mirror, as people expect of a self-view */
    });
    auto failed = [this](const QString &msg) {
        camera_->setChecked(false);
        share_->setChecked(false);
        camera_->setToolTip(msg);
        stopCamera();
        core_->call("set_call_camera", {{"on", false}});
    };
#ifdef VC_HAVE_SCREENCAPTURE
    if (screen) {
        screen_ = new QScreenCapture(this);
        screen_->setScreen(QGuiApplication::primaryScreen());
        session_->setScreenCapture(screen_);
        connect(screen_, &QScreenCapture::errorOccurred, this, [failed](QScreenCapture::Error, const QString &msg) { failed(msg); });
        screen_->start();
    } else
#else
    if (screen) { failed("Screen sharing needs Qt 6.5 or newer"); return; } else
#endif
    {
        const auto devices = QMediaDevices::videoInputs();
        QCameraDevice device = devices.first();
        const QString chosen = core_->pref("callCamera");
        for (const QCameraDevice &dev : devices) if (!chosen.isEmpty() && dev.description() == chosen) device = dev;
        cam_ = new QCamera(device, this);
        session_->setCamera(cam_);
        connect(cam_, &QCamera::errorOccurred, this, [failed](QCamera::Error, const QString &msg) { failed(msg); });
        cam_->start();
    }
    core_->call("set_call_camera", {{"on", true}});
}

void CallDialog::cameraOn(bool on)
{
    share_->setChecked(false);
    if (!on) {
        stopCamera();
        core_->call("set_call_camera", {{"on", false}});
        return;
    }
    if (QMediaDevices::videoInputs().isEmpty()) {
        camera_->setChecked(false);
        camera_->setEnabled(false);
        camera_->setToolTip("No camera found");
        return;
    }
    startSource(false);
}

/* Show the screen in place of the camera (the other side sees it as our video). */
void CallDialog::shareOn(bool on)
{
    camera_->setChecked(false);
    if (!on) {
        stopCamera();
        core_->call("set_call_camera", {{"on", false}});
        return;
    }
    startSource(true);
}

void CallDialog::tick()
{
    if (state_ != "connected") return;
    const int s = int(since_.elapsed() / 1000);
    status_->setText(QString("Connected  %1:%2").arg(s / 60).arg(s % 60, 2, 10, QChar('0')));
}

void CallDialog::setState(const QJsonObject &s)
{
    const QString st = s["state"].toString();
    ring(st, s["incoming"].toBool());
    const bool was = state_ == "connected";
    state_ = st;
    name_->setText(s["name"].toString().isEmpty() ? s["user_id"].toString() : s["name"].toString());
    const bool incoming = s["incoming"].toBool();
    answer_->setVisible(st == "incoming");
    answer_->setEnabled(true);
    const bool live = st == "connecting" || st == "connected";
    mute_->setVisible(live);
    mute_->setChecked(s["muted"].toBool());
    hasVideo_ = s["video"].toBool();
    remoteShows_ = s["remote_video"].toBool(true);
    setWindowTitle(hasVideo_ ? "Video call" : "Voice call");
    video_->setVisible(hasVideo_ && st != "ended");
    camera_->setVisible(hasVideo_ && live);
    share_->setVisible(hasVideo_ && live);
    addVideo_->setVisible(!hasVideo_ && st == "connected");
    if (st == "incoming" || st == "outgoing") { gotRemote_ = false; video_->setRemote(QImage()); camera_->setChecked(false); share_->setChecked(false); }
    if (hasVideo_) {
        setMinimumWidth(560);
        video_->setNote(!live ? QString() : !remoteShows_ ? name_->text() + "'s camera is off" : gotRemote_ ? QString() : "Waiting for the picture...");
    }
    hangup_->setText(st == "incoming" ? "Decline" : "Hang up");
    hangup_->setVisible(st != "ended");
    if (st == "incoming") status_->setText("Incoming call");
    else if (st == "outgoing") status_->setText("Calling...");
    else if (st == "connecting") status_->setText("Connecting...");
    else if (st == "connected") { if (!was) since_.start(); tick(); timer_->start(); }
    else if (st == "ended") {
        timer_->stop();
        stopCamera();
        const QString r = s["reason"].toString();
        status_->setText(r == "user_busy" ? "Busy" : r == "invite_timeout" ? (incoming ? "Missed call" : "No answer") : r == "rejected" ? "Declined" : r == "ice_failed" || r == "ice_timeout" || r == "error" ? "The call failed" : "Call ended");
        QTimer::singleShot(2500, this, &QDialog::close);
    }
}

void CallDialog::closeEvent(QCloseEvent *ev)
{
    stopCamera();
    if (ringer_) ringer_->stop();
    if (state_ != "ended") core_->call("hangup_call");
    ev->accept();
}

/* Empties a layout. A nested QLayout is itself the item takeAt() returns: deleting it and then "its item" would delete it twice. */
static void clearLayout(QLayout *l)
{
    while (QLayoutItem *it = l->takeAt(0)) {
        if (QWidget *w = it->widget()) { delete w; delete it; }
        else if (QLayout *sub = it->layout()) { clearLayout(sub); delete sub; }
        else delete it;
    }
}

void VerifyDialog::setState(const QJsonObject &s)
{
    const QString st = S(s, "state");
    if (st == "idle") { finished_ = true; hide(); return; }
    finished_ = st == "done" || st == "cancelled";
    clearLayout(lay_);
    const QString user = S(s, "user");
    const bool own = user.isEmpty() || user == me_;
    auto text = [this](const QString &t, const QString &css = QString()) {
        auto *l = new QLabel(t);
        l->setWordWrap(true);
        if (!css.isEmpty()) l->setStyleSheet(css);
        lay_->addWidget(l);
    };
    auto buttons = [this](std::initializer_list<std::pair<QString, std::function<void()>>> bs, bool primaryFirst) {
        auto *row = new QHBoxLayout;
        bool first = true;
        for (const auto &b : bs) {
            auto *btn = new QPushButton(b.first);
            if (first && primaryFirst) btn->setDefault(true);
            first = false;
            auto fn = b.second;
            connect(btn, &QPushButton::clicked, this, [fn] { fn(); });
            row->addWidget(btn);
        }
        row->addStretch(1);
        lay_->addLayout(row);
    };
    auto *title = new QLabel(own ? "Verify this session" : "Verify " + user);
    QFont tf = title->font();
    tf.setPointSizeF(tf.pointSizeF() * 1.4);
    tf.setBold(true);
    title->setFont(tf);
    lay_->addWidget(title);
    const QString muted = "color:gray";
    Core *c = core_;
    if (st == "incoming") {
        text(own ? "Your other session wants to verify this one. Only accept if you started this yourself."
                 : user + " wants to verify each other. You will compare a few emoji, ideally over a call or in person. Only accept if you expected this.", muted);
        buttons({{"Accept", [c] { c->call("accept_verification"); }}, {"Decline", [c] { c->call("cancel_verification"); }}}, true);
    } else if (st == "waiting") {
        text(own ? "Waiting for your other session to accept. Open Element or another qVector on it and accept the verification request."
                 : "Waiting for " + user + " to accept the request in their client...", muted);
        buttons({{"Cancel", [c] { c->call("cancel_verification"); }}}, false);
    } else if (st == "qr") { /* the other device scans this code (a phone's camera); or compare emoji instead */
        text(own ? "On your other session choose \"Scan QR code\" and point its camera at this code."
                 : "Ask " + user + " to scan this code with their device (choose \"Scan QR code\" in their client).", muted);
        const QJsonObject qr = s["qr"].toObject();
        const int n = qr["size"].toInt(), scale = 6, quiet = 4;
        if (n > 0) {
            QImage img((n + 2 * quiet) * scale, (n + 2 * quiet) * scale, QImage::Format_RGB32);
            img.fill(Qt::white); /* a QR code is black on white whatever the theme: scanners need the contrast and the quiet border */
            const QJsonArray rows = qr["rows"].toArray();
            for (int y = 0; y < n && y < rows.size(); y++) {
                const QString r = rows[y].toString();
                for (int x = 0; x < n && x < r.size(); x++)
                    if (r[x] == '1') for (int dy = 0; dy < scale; dy++) for (int dx = 0; dx < scale; dx++) img.setPixel((x + quiet) * scale + dx, (y + quiet) * scale + dy, qRgb(0, 0, 0));
            }
            auto *l = new QLabel;
            l->setPixmap(QPixmap::fromImage(img));
            l->setAlignment(Qt::AlignCenter);
            lay_->addWidget(l);
        }
        buttons({{"Compare emoji instead", [c] { c->call("emoji_verification"); }}, {"Cancel", [c] { c->call("cancel_verification"); }}}, false);
    } else if (st == "qr_scanned") {
        text(own ? "Your other session scanned the code. Does it say it scanned this session's code? Then confirm."
                 : user + " scanned the code. Confirm if they say it worked.", muted);
        buttons({{"Confirm", [c] { c->call("confirm_verification"); }}, {"Cancel", [c] { c->call("cancel_verification"); }}}, true);
    } else if (st == "emoji" || st == "confirmed") {
        text(own ? "Check that these emoji appear, in the same order, on your other session."
                 : "Check that these emoji appear, in the same order, for " + user + ". Compare them over a call or in person, not in this chat.", muted);
        auto *row = new QHBoxLayout;
        row->addStretch(1);
        for (const QJsonValue &ev : s["emoji"].toArray()) {
            const QJsonArray e = ev.toArray();
            auto *col = new QVBoxLayout;
            auto *el = new QLabel(e.at(0).toString());
            QFont f = el->font();
            f.setPointSize(30);
            if (!emojiFamily().isEmpty()) f.setFamilies({emojiFamily(), f.family()});
            el->setFont(f);
            el->setAlignment(Qt::AlignCenter);
            auto *nl = new QLabel(e.at(1).toString());
            nl->setAlignment(Qt::AlignCenter);
            nl->setStyleSheet(muted);
            col->addWidget(el);
            col->addWidget(nl);
            row->addLayout(col);
        }
        row->addStretch(1);
        lay_->addLayout(row);
        const QJsonArray dec = s["decimals"].toArray();
        if (dec.size() == 3) text(QString("Or compare the numbers:  %1   %2   %3").arg(dec[0].toInt()).arg(dec[1].toInt()).arg(dec[2].toInt()), muted);
        if (st == "emoji") buttons({{"They match", [c] { c->call("confirm_verification"); }}, {"They don't match", [c] { c->call("cancel_verification"); }}}, true);
        else { text("Waiting for the other side to confirm...", muted); buttons({{"Cancel", [c] { c->call("cancel_verification"); }}}, false); }
    } else if (st == "done") {
        text(own ? "Verified: the other session confirmed this one." : user + " is verified.", "color:" + okColor(qApp->palette()).name());
        text(own ? "This session is now cross-signed, so your other sessions and contacts will trust it." : "Their messages from verified sessions now show no warning, and you are warned if their identity ever changes.", muted);
        buttons({{"Done", [this] { close(); }}}, true);
    } else if (st == "cancelled") {
        text(S(s, "reason").isEmpty() ? "Verification cancelled." : S(s, "reason"), "color:" + errorColor(qApp->palette()).name());
        buttons({{"Close", [this] { close(); }}}, false);
    }
    show();
    raise();
}

/* ---------- picture viewer ---------- */

ImageViewer::ImageViewer(const QString &path, const QString &name, QWidget *parent) : QDialog(parent)
{
    setWindowTitle(name.isEmpty() ? QString("Image") : QFileInfo(name).fileName());
    QImageReader rd(path);
    rd.setAutoTransform(true);
    image_ = rd.read();
    resize(900, 640);
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(0, 0, 0, 0);
    v->setSpacing(0);
    auto *bar = new QToolBar;
    bar->setToolButtonStyle(Qt::ToolButtonTextOnly); /* words, not symbols: a symbol font could draw coloured emoji */
    bar->setMovable(false);
    bar->addAction("Save as...", this, [this] { emit saveRequested(); })->setToolTip("Save the picture to a file");
    bar->addAction("Copy", this, [this] { if (!image_.isNull()) QApplication::clipboard()->setImage(image_); })->setToolTip("Copy the picture to the clipboard");
    bar->addSeparator();
    bar->addAction("Zoom out", this, [this] { zoomBy(1 / 1.25); })->setToolTip("Zoom out (-)");
    bar->addAction("Zoom in", this, [this] { zoomBy(1.25); })->setToolTip("Zoom in (+, or Ctrl+wheel)");
    bar->addAction("Fit", this, [this] { zoom_ = 0; render(); })->setToolTip("Fit the picture to the window (0)");
    bar->addAction("100%", this, [this] { zoom_ = 1; render(); })->setToolTip("Actual size (1)");
    auto *spacer = new QWidget;
    spacer->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    bar->addWidget(spacer);
    info_ = new QLabel;
    info_->setContentsMargins(0, 0, 8, 0);
    bar->addWidget(info_);
    v->addWidget(bar);
    label_ = new QLabel("Loading...");
    label_->setAlignment(Qt::AlignCenter);
    area_ = new QScrollArea;
    area_->setWidgetResizable(true);
    area_->setWidget(label_);
    area_->setFrameShape(QFrame::NoFrame);
    area_->viewport()->installEventFilter(this);
    v->addWidget(area_, 1);
    render();
}

void ImageViewer::zoomBy(double factor)
{
    if (image_.isNull()) return;
    double cur = zoom_;
    if (cur <= 0) { /* from "fit": start at the scale that fit */
        const QSize avail = area_->viewport()->size();
        cur = qMin(1.0, qMin(double(avail.width()) / image_.width(), double(avail.height()) / image_.height()));
    }
    zoom_ = qBound(0.05, cur * factor, 16.0);
    render();
}

void ImageViewer::resizeEvent(QResizeEvent *e)
{
    QDialog::resizeEvent(e);
    if (zoom_ <= 0) render();
}

void ImageViewer::keyPressEvent(QKeyEvent *e)
{
    switch (e->key()) {
    case Qt::Key_Plus: case Qt::Key_Equal: zoomBy(1.25); break;
    case Qt::Key_Minus: zoomBy(1 / 1.25); break;
    case Qt::Key_0: zoom_ = 0; render(); break;
    case Qt::Key_1: zoom_ = 1; render(); break;
    default: QDialog::keyPressEvent(e); return;
    }
    e->accept();
}

bool ImageViewer::eventFilter(QObject *obj, QEvent *ev)
{
    if (obj == area_->viewport() && ev->type() == QEvent::Wheel) {
        auto *w = static_cast<QWheelEvent *>(ev);
        if (w->modifiers() & Qt::ControlModifier) { zoomBy(w->angleDelta().y() > 0 ? 1.15 : 1 / 1.15); return true; }
    }
    return QDialog::eventFilter(obj, ev);
}

void ImageViewer::render()
{
    if (image_.isNull()) { label_->setText("The picture could not be loaded."); return; }
    const QSize avail = area_->viewport()->size() - QSize(4, 4);
    const double dpr = devicePixelRatioF();
    double k = zoom_;
    if (k <= 0) k = qMin(1.0, qMin(double(avail.width()) / image_.width(), double(avail.height()) / image_.height()));
    const QSize target(qMax(1, int(image_.width() * k)), qMax(1, int(image_.height() * k)));
    QPixmap pm = QPixmap::fromImage(target == image_.size() ? image_ : image_.scaled(target * dpr, Qt::IgnoreAspectRatio, Qt::SmoothTransformation));
    pm.setDevicePixelRatio(target == image_.size() ? 1.0 : dpr);
    label_->setPixmap(pm);
    info_->setText(QString("%1 x %2   %3%").arg(image_.width()).arg(image_.height()).arg(int(k * 100 + 0.5)));
}

/* ---------- text viewer ---------- */

TextViewer::TextViewer(const QString &name, const QString &text, qint64 size, bool truncated, QWidget *parent) : QDialog(parent)
{
    setWindowTitle(name.isEmpty() ? QString("Text file") : QFileInfo(name).fileName());
    resize(820, 620);
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(0, 0, 0, 0);
    v->setSpacing(0);
    auto *bar = new QToolBar;
    bar->setToolButtonStyle(Qt::ToolButtonTextOnly);
    bar->setMovable(false);
    bar->addAction("Save as...", this, [this] { emit saveRequested(); })->setToolTip("Save the file");
    auto *edit = new QPlainTextEdit;
    edit->setReadOnly(true);
    edit->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    edit->setLineWrapMode(QPlainTextEdit::NoWrap);
    edit->setPlainText(text);
    edit->setFrameShape(QFrame::NoFrame);
    bar->addAction("Copy all", this, [edit] { QApplication::clipboard()->setText(edit->toPlainText()); })->setToolTip("Copy the whole text to the clipboard");
    QAction *wrap = bar->addAction("Wrap lines");
    wrap->setCheckable(true);
    wrap->setToolTip("Wrap long lines");
    connect(wrap, &QAction::toggled, edit, [edit](bool on) { edit->setLineWrapMode(on ? QPlainTextEdit::WidgetWidth : QPlainTextEdit::NoWrap); });
    bar->addSeparator();
    auto *find = new QLineEdit;
    find->setPlaceholderText("Find");
    find->setClearButtonEnabled(true);
    find->setMaximumWidth(220);
    bar->addWidget(find);
    connect(find, &QLineEdit::returnPressed, edit, [edit, find] { if (!edit->find(find->text())) { edit->moveCursor(QTextCursor::Start); edit->find(find->text()); } }); /* Enter: next hit, wrapping around */
    auto *spacer = new QWidget;
    spacer->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    bar->addWidget(spacer);
    const int lines = text.count('\n') + (text.isEmpty() || text.endsWith('\n') ? 0 : 1);
    auto *info = new QLabel(QString("%1 lines   %2%3").arg(lines).arg(size < 1024 ? QString::number(size) + " B" : size < 1024 * 1024 ? QString::number((size + 1023) / 1024) + " KB" : QString::number(double(size) / (1024 * 1024), 'f', 1) + " MB", truncated ? "   (only the start is shown)" : QString()));
    info->setContentsMargins(0, 0, 8, 0);
    bar->addWidget(info);
    v->addWidget(bar);
    v->addWidget(edit, 1);
    new QShortcut(QKeySequence::Find, this, [find] { find->setFocus(); find->selectAll(); });
}

/* ---------- room settings ---------- */

RoomSettingsDialog::RoomSettingsDialog(Core *core, const QJsonObject &d, QWidget *parent) : QDialog(parent)
{
    setWindowTitle("Room settings");
    setMinimumWidth(440);
    auto *v = new QVBoxLayout(this);
    auto *form = new QFormLayout;
    auto *name = new QLineEdit(S(d, "name")), *topic = new QLineEdit(S(d, "topic"));
    form->addRow("Name", name);
    form->addRow("Topic", topic);
    v->addLayout(form);
    auto *row = new QHBoxLayout;
    auto *save = new QPushButton("Save");
    auto *cancel = new QPushButton("Cancel");
    save->setDefault(true);
    row->addWidget(save);
    row->addWidget(cancel);
    row->addStretch(1);
    v->addLayout(row);
    connect(cancel, &QPushButton::clicked, this, &QDialog::reject);
    connect(save, &QPushButton::clicked, this, [=] { core->call("room_action", {{"kind", "texts"}, {"a", name->text()}, {"b", topic->text()}}); accept(); });
}

void showAbout(QWidget *parent)
{
    QMessageBox::about(parent, "About qVector",
                       "<b>qVector</b><br>A Matrix client: Qt6 interface over a Rust core (matrix-sdk).<br>"
                       "End-to-end encryption, key backup, cross-signing and device verification are built in.");
}

}
