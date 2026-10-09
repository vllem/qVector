#include "qt/memberlist.h"
#include "qt/avatar.h"
#include <QApplication>
#include <QClipboard>
#include <QJsonArray>
#include <QMenu>
#include <QPainter>

namespace vc {

MemberList::MemberList(QWidget *parent) : QListWidget(parent)
{
    setFrameShape(QFrame::NoFrame);
    setIconSize(QSize(22, 22));
    setSelectionMode(QAbstractItemView::NoSelection);
    setMinimumWidth(150);
    setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    setContextMenuPolicy(Qt::CustomContextMenu);
    connect(this, &QWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        QListWidgetItem *it = itemAt(pos);
        if (!it || it->data(Qt::UserRole).toString().isEmpty()) return;
        const QString id = it->data(Qt::UserRole).toString();
        const bool banned = it->data(Qt::UserRole + 1).toBool(), canKick = it->data(Qt::UserRole + 2).toBool(), canBan = it->data(Qt::UserRole + 3).toBool(), verified = it->data(Qt::UserRole + 4).toBool();
        QMenu menu(this);
        if (id != me_) {
            menu.addAction("Message", this, [this, id] { emit messageRequested(id); });
            if (!verified) menu.addAction("Verify...", this, [this, id] { emit verifyRequested(id); });
        }
        menu.addAction("Copy user ID", this, [id] { QApplication::clipboard()->setText(id); });
        bool sep = false;
        auto add = [&](const QString &text, const QString &action) {
            if (!sep) { menu.addSeparator(); sep = true; }
            menu.addAction(text, this, [this, action, id] { emit moderationRequested(action, id); });
        };
        if (banned) { if (canBan) add("Unban", "unban"); }
        else if (id != me_) {
            if (canKick) add("Kick...", "kick");
            if (canBan) add("Ban...", "ban");
            if (canSetRoles_) { add("Make admin", "role:admin"); add("Make moderator", "role:moderator"); add("Remove role", "role:member"); }
        }
        menu.exec(viewport()->mapToGlobal(pos));
    });
}

void MemberList::setPresence(const QJsonObject &states)
{
    if (states == presence_) return;
    const QJsonObject before = presence_;
    presence_ = states;
    if (last_.isEmpty()) return;
    if (itemById_.isEmpty()) { sig_.clear(); refresh(last_); return; }
    /* Heartbeats flip a handful of people at a time: redo those rows, not all of a 20 000-person list. Both objects are sorted by key, so
       one walk side by side finds the differences without a look-up per person. */
    QStringList changed;
    auto a = before.constBegin(), b = presence_.constBegin();
    while (a != before.constEnd() || b != presence_.constEnd()) {
        if (b == presence_.constEnd() || (a != before.constEnd() && a.key() < b.key())) { changed << a.key(); ++a; }
        else if (a == before.constEnd() || b.key() < a.key()) { changed << b.key(); ++b; }
        else { if (a.value().toString() != b.value().toString()) changed << a.key(); ++a; ++b; }
    }
    for (const QString &id : changed) {
        const auto it = itemById_.constFind(id);
        if (it != itemById_.constEnd()) decorate(it.value(), memberById_.value(id), false);
    }
    sig_.clear(); /* the signature includes presence; the next refresh() must not trust the old one */
}

void MemberList::decorate(QListWidgetItem *it, const QJsonObject &m, bool banned)
{
    const QPalette pal = palette();
    const QString id = m["user_id"].toString(), name = m["name"].toString();
    QPixmap pic = profilePixmap(m["avatar_path"].toString(), id, name, 22, devicePixelRatioF(), pal);
    const bool verified = m["verified"].toBool();
    if (verified) { /* a small shield in the corner of the picture */
        QPainter pp(&pic);
        pp.drawPixmap(QPointF(0, 0), shieldPixmap(0, 11, devicePixelRatioF()));
    }
    const QString state = presence_[id].toString();
    if (!banned && (state == "online" || state == "unavailable")) { /* a coloured dot in the lower right corner of the picture */
        QPainter pp(&pic);
        pp.setRenderHint(QPainter::Antialiasing);
        const qreal r = 4.5, c = 22 - r - 0.5;
        pp.setPen(QPen(pal.color(QPalette::Base), 1.6));
        pp.setBrush(state == "online" ? QColor("#2e9e4f") : QColor("#d89a1c"));
        pp.drawEllipse(QPointF(c, c), r - 0.8, r - 0.8);
    }
    it->setIcon(QIcon(pic));
    it->setText(name);
    QString tip = id;
    if (state == "online") tip += "\nOnline"; else if (state == "unavailable") tip += "\nAway";
    if (verified) tip += "\nVerified";
    if (!m["role"].toString().isEmpty()) tip += "\n" + m["role"].toString();
    it->setToolTip(tip);
}

void MemberList::refresh(const QJsonObject &d)
{
    last_ = d;
    canSetRoles_ = d["can_set_roles"].toBool();
    struct Row { QJsonObject m; bool banned; int rank; };
    QList<Row> rows;
    auto rankOf = [](const QString &role) { return role == "Admin" || role == "Creator" ? 0 : role == "Moderator" ? 1 : 2; };
    for (const QJsonValue &v : d["members"].toArray()) rows.append({v.toObject(), false, rankOf(v.toObject()["role"].toString())});
    for (const QJsonValue &v : d["banned"].toArray()) rows.append({v.toObject(), true, 4});
    QString sig = d["id"].toString();
    for (const Row &x : rows) sig += "|" + x.m["user_id"].toString() + ":" + x.m["name"].toString() + ":" + QString::number(x.rank) + QString::number(x.m["verified"].toBool()) + QString::number(x.m["can_kick"].toBool()) + x.m["avatar_path"].toString() + presence_[x.m["user_id"].toString()].toString();
    if (sig == sig_) return;
    sig_ = sig;
    clear();
    itemById_.clear(); memberById_.clear();
    setUpdatesEnabled(false); /* 20 000 rows are added; repaint once */
    const QPalette pal = palette();
    int lastRank = -1;
    const bool staff = !rows.isEmpty() && rows.first().rank < 2;
    for (const Row &x : rows) {
        if (x.rank != lastRank) {
            static const char *const HEAD[] = {"Admins", "Moderators", "Members", "Invited", "Banned"};
            lastRank = x.rank;
            if (lastRank != 2 || staff) {
                auto *h = new QListWidgetItem(HEAD[lastRank]);
                h->setFlags(Qt::NoItemFlags);
                h->setForeground(pal.color(QPalette::PlaceholderText));
                addItem(h);
            }
        }
        const QString id = x.m["user_id"].toString();
        auto *it = new QListWidgetItem;
        decorate(it, x.m, x.banned);
        it->setData(Qt::UserRole, id);
        it->setData(Qt::UserRole + 1, x.banned);
        it->setData(Qt::UserRole + 2, x.m["can_kick"].toBool());
        it->setData(Qt::UserRole + 3, x.m["can_ban"].toBool());
        it->setData(Qt::UserRole + 4, x.m["verified"].toBool());
        if (!x.banned) { itemById_.insert(id, it); memberById_.insert(id, x.m); }
        if (x.banned) it->setForeground(pal.color(QPalette::PlaceholderText));
        addItem(it);
    }
    setUpdatesEnabled(true);
}

}
