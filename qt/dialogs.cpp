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
#include <QKeyEvent>
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
