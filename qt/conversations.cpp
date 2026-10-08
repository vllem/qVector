#include "qt/conversations.h"
#include "qt/avatar.h"
#include <QDateTime>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QJsonObject>
#include <QMessageBox>
#include <QRegularExpression>
#include <QTextBrowser>
#include <QTimer>
#include <QVBoxLayout>

namespace vc {

static QString S(const QJsonObject &o, const char *k) { return o.value(QLatin1String(k)).toString(); }

static bool looksLikeUserId(const QString &s)
{
    return s.startsWith('@') && s.contains(':') && s.size() > 3 && !s.contains(' ');
}

/* ---------- start a direct message ---------- */

StartDmDialog::StartDmDialog(Core *core, QWidget *parent) : QDialog(parent), core_(core)
{
    setWindowTitle("Start a direct message");
    setMinimumWidth(440);
    auto *v = new QVBoxLayout(this);
    who_ = new QLineEdit;
    who_->setPlaceholderText("Name, or a full Matrix id such as @alice:example.org");
    hits_ = new QListWidget;
    hits_->setIconSize(QSize(28, 28));
    hits_->setMinimumHeight(180);
    go_ = new QPushButton("Start chat");
    go_->setDefault(true);
    go_->setEnabled(false);
    auto *row = new QHBoxLayout;
    row->addStretch(1);
    row->addWidget(go_);
    v->addWidget(new QLabel("Who do you want to talk to? (The chat is end-to-end encrypted.)"));
    v->addWidget(who_);
    v->addWidget(hits_, 1);
    v->addLayout(row);
    debounce_.setSingleShot(true);
    connect(&debounce_, &QTimer::timeout, this, [this] { core_->call("search_users", {{"term", who_->text().trimmed()}}); });
    connect(who_, &QLineEdit::textChanged, this, [this] { debounce_.start(300); rebuild(); });
    connect(hits_, &QListWidget::currentRowChanged, this, [this] { go_->setEnabled(!chosen().isEmpty()); });
    connect(hits_, &QListWidget::itemDoubleClicked, this, [this] { go_->click(); });
    connect(who_, &QLineEdit::returnPressed, this, [this] { if (hits_->currentRow() < 0 && hits_->count()) hits_->setCurrentRow(0); go_->click(); });
    connect(go_, &QPushButton::clicked, this, [this] {
        const QString id = chosen();
        if (id.isEmpty()) return;
        core_->call("start_dm", {{"user_id", id}});
        accept();
    });
}

QString StartDmDialog::chosen() const
{
    QListWidgetItem *it = hits_->currentItem();
    return it ? it->data(Qt::UserRole).toString() : QString();
}

void StartDmDialog::setUsers(const QJsonArray &users) { users_ = users; rebuild(); }

void StartDmDialog::rebuild()
{
    const QString typed = who_->text().trimmed(), keep = chosen();
    hits_->clear();
    const QPalette pal = palette();
    if (looksLikeUserId(typed)) { /* an id typed in full can always be used, found or not */
        auto *it = new QListWidgetItem(QIcon(profilePixmap(QString(), typed, typed, 28, devicePixelRatioF(), pal)), typed);
        it->setData(Qt::UserRole, typed);
        hits_->addItem(it);
    }
    for (const QJsonValue &v : users_) {
        const QJsonObject h = v.toObject();
        const QString id = S(h, "user_id"), name = S(h, "name");
        if (id == typed) continue;
        auto *it = new QListWidgetItem(QIcon(profilePixmap(S(h, "avatar_path"), id, name.isEmpty() ? id : name, 28, devicePixelRatioF(), pal)), name.isEmpty() ? id : name + "  (" + id + ")");
        it->setData(Qt::UserRole, id);
        hits_->addItem(it);
    }
    for (int i = 0; i < hits_->count(); i++) if (hits_->item(i)->data(Qt::UserRole).toString() == keep) hits_->setCurrentRow(i);
    if (hits_->currentRow() < 0 && hits_->count()) hits_->setCurrentRow(0);
    go_->setEnabled(!chosen().isEmpty());
}

/* ---------- create a room ---------- */

CreateRoomDialog::CreateRoomDialog(Core *core, QWidget *parent) : QDialog(parent)
{
    setWindowTitle("Create a room");
    setMinimumWidth(440);
    auto *form = new QFormLayout;
    auto *name = new QLineEdit, *topic = new QLineEdit, *invites = new QLineEdit;
    auto *pub = new QCheckBox("Public room (listed in the room directory, anyone can join)");
    auto *enc = new QCheckBox("End-to-end encrypted");
    enc->setChecked(true);
    invites->setPlaceholderText("@alice:example.org, @bob:example.org");
    form->addRow("Name", name);
    form->addRow("Topic", topic);
    form->addRow("Invite", invites);
    form->addRow(pub);
    form->addRow(enc);
    auto *go = new QPushButton("Create room");
    go->setDefault(true);
    auto *v = new QVBoxLayout(this);
    v->addLayout(form);
    v->addWidget(go, 0, Qt::AlignRight);
    connect(pub, &QCheckBox::toggled, this, [enc](bool on) { enc->setChecked(!on); }); /* public rooms are usually not encrypted; the box stays changeable */
    connect(go, &QPushButton::clicked, this, [=] {
        QJsonArray ids;
        for (const QString &s : invites->text().split(QRegularExpression("[,;\\s]+"), Qt::SkipEmptyParts)) {
            if (!looksLikeUserId(s.trimmed())) { QMessageBox::warning(this, "Create a room", s + " is not a Matrix user id (like @alice:example.org)."); return; }
            ids.append(s.trimmed());
        }
        if (name->text().trimmed().isEmpty()) { QMessageBox::warning(this, "Create a room", "Give the room a name."); return; }
        core->call("create_room", {{"name", name->text().trimmed()}, {"topic", topic->text().trimmed()}, {"encrypted", enc->isChecked()}, {"public", pub->isChecked()}, {"invites", ids}});
        accept();
    });
}

/* ---------- browse the public rooms ---------- */

BrowseRoomsDialog::BrowseRoomsDialog(Core *core, QWidget *parent, const QString &spaceId) : QDialog(parent), core_(core), spaceId_(spaceId)
{
    setWindowTitle(spaceId.isEmpty() ? "Browse public rooms" : "Rooms in this space");
    resize(560, 460);
    auto *v = new QVBoxLayout(this);
    auto *row = new QHBoxLayout;
    term_ = new QLineEdit;
    term_->setPlaceholderText("Search rooms");
    server_ = new QLineEdit;
    server_->setPlaceholderText("server (default: yours)");
    server_->setMaximumWidth(190);
    auto *find = new QPushButton("Search");
    row->addWidget(term_, 1);
    row->addWidget(server_);
    row->addWidget(find);
    list_ = new QListWidget;
    list_->setIconSize(QSize(32, 32));
    status_ = new QLabel;
    join_ = new QPushButton("Join");
    join_->setEnabled(false);
    v->addLayout(row);
    v->addWidget(list_, 1);
    auto *bottom = new QHBoxLayout;
    bottom->addWidget(status_, 1);
    bottom->addWidget(join_);
    v->addLayout(bottom);
    if (!spaceId.isEmpty()) { term_->hide(); server_->hide(); find->setText("Refresh"); find->setMaximumWidth(110); row->insertStretch(0, 1); }
    connect(find, &QPushButton::clicked, this, [this] { search(); });
    connect(term_, &QLineEdit::returnPressed, this, [this] { search(); });
    connect(server_, &QLineEdit::returnPressed, this, [this] { search(); });
    connect(list_, &QListWidget::currentRowChanged, this, [this](int r) { join_->setEnabled(r >= 0); });
    connect(list_, &QListWidget::itemDoubleClicked, this, [this] { joinSelected(); });
    connect(join_, &QPushButton::clicked, this, [this] { joinSelected(); });
    search();
}

void BrowseRoomsDialog::search()
{
    status_->setText("Searching...");
    if (!spaceId_.isEmpty()) { core_->call("space_rooms", {{"space_id", spaceId_}}); return; }
    core_->call("public_rooms", {{"term", term_->text().trimmed()}, {"server", server_->text().trimmed()}});
}

void BrowseRoomsDialog::joinSelected()
{
    QListWidgetItem *it = list_->currentItem();
    if (!it) return;
    if (it->data(Qt::UserRole + 2).toBool()) return; /* already in */
    QJsonObject args{{"address", it->data(Qt::UserRole).toString()}};
    if (!spaceId_.isEmpty()) args["via"] = QJsonArray{spaceId_.section(':', 1)}; /* the server of the space knows its rooms */
    core_->call("join_room", args);
    status_->setText("Joining " + it->data(Qt::UserRole + 1).toString() + "...");
    if (!spaceId_.isEmpty()) QTimer::singleShot(2500, this, [this] { search(); });
}

void BrowseRoomsDialog::setRooms(const QJsonArray &rooms)
{
    list_->clear();
    const QPalette pal = palette();
    for (const QJsonValue &v : rooms) {
        const QJsonObject d = v.toObject();
        const QString title = !S(d, "name").isEmpty() ? S(d, "name") : !S(d, "alias").isEmpty() ? S(d, "alias") : S(d, "room_id");
        auto *it = new QListWidgetItem(QIcon(profilePixmap(QString(), S(d, "room_id"), title, 32, devicePixelRatioF(), pal)),
                                       QString("%1  -  %2 members%3\n%4").arg(title).arg(d["members"].toInt()).arg(d["joined"].toBool() ? "  -  joined" : QString()).arg(S(d, "topic").simplified().left(120)));
        it->setData(Qt::UserRole + 2, d["joined"].toBool());
        if (d["joined"].toBool()) it->setForeground(pal.color(QPalette::PlaceholderText));
        it->setData(Qt::UserRole, !S(d, "alias").isEmpty() ? S(d, "alias") : S(d, "room_id")); /* an alias also works for rooms we cannot see into yet */
        it->setData(Qt::UserRole + 1, title);
        list_->addItem(it);
    }
    status_->setText(rooms.isEmpty() ? "No rooms found" : QString("%1 rooms").arg(rooms.size()));
}

/* ---------- forward ---------- */

ForwardDialog::ForwardDialog(Core *core, const QJsonArray &rooms, const QString &eventId, QWidget *parent) : QDialog(parent)
{
    setWindowTitle("Forward message");
    setMinimumSize(380, 420);
    auto *v = new QVBoxLayout(this);
    auto *filter = new QLineEdit;
    filter->setPlaceholderText("Search rooms and people");
    auto *list = new QListWidget;
    list->setSelectionMode(QAbstractItemView::ExtendedSelection);
    for (const QJsonValue &rv : rooms) {
        const QJsonObject r = rv.toObject();
        if (r["invite"].toBool()) continue;
        auto *it = new QListWidgetItem(S(r, "title"));
        it->setData(Qt::UserRole, S(r, "id"));
        list->addItem(it);
    }
    auto *go = new QPushButton("Forward");
    go->setDefault(true);
    v->addWidget(new QLabel("Send a copy to:"));
    v->addWidget(filter);
    v->addWidget(list, 1);
    v->addWidget(go, 0, Qt::AlignRight);
    connect(filter, &QLineEdit::textChanged, this, [list](const QString &t) {
        for (int i = 0; i < list->count(); i++) list->item(i)->setHidden(!list->item(i)->text().contains(t, Qt::CaseInsensitive));
    });
    connect(go, &QPushButton::clicked, this, [=] {
        QJsonArray ids;
        for (QListWidgetItem *it : list->selectedItems()) ids.append(it->data(Qt::UserRole).toString());
        if (ids.isEmpty()) return;
        core->call("forward", {{"event_id", eventId}, {"room_ids", ids}});
        accept();
    });
    connect(list, &QListWidget::itemDoubleClicked, this, [go] { go->click(); });
}

/* ---------- edit history ---------- */

void showEditHistory(QWidget *parent, const QJsonArray &revisions)
{
    auto *dlg = new QDialog(parent);
    dlg->setAttribute(Qt::WA_DeleteOnClose);
    dlg->setWindowTitle("Edit history");
    dlg->resize(480, 340);
    auto *v = new QVBoxLayout(dlg);
    auto *tb = new QTextBrowser;
    QString html;
    int i = 0;
    for (const QJsonValue &rv : revisions) {
        const QJsonObject r = rv.toObject();
        html += "<p><b>" + QString(i == 0 ? "Original" : "Edit " + QString::number(i)) + "</b> <span style=\"color:gray\">" + S(r, "time").toHtmlEscaped() + "</span><br>" +
                S(r, "text").toHtmlEscaped().replace('\n', "<br>") + "</p>";
        i++;
    }
    tb->setHtml(html);
    v->addWidget(tb);
    dlg->show();
}

}
