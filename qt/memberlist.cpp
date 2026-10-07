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

void MemberList::refresh(const QJsonObject &d)
{
    canSetRoles_ = d["can_set_roles"].toBool();
    struct Row { QJsonObject m; bool banned; int rank; };
    QList<Row> rows;
    auto rankOf = [](const QString &role) { return role == "Admin" || role == "Creator" ? 0 : role == "Moderator" ? 1 : 2; };
    for (const QJsonValue &v : d["members"].toArray()) rows.append({v.toObject(), false, rankOf(v.toObject()["role"].toString())});
    for (const QJsonValue &v : d["banned"].toArray()) rows.append({v.toObject(), true, 4});
    QString sig = d["id"].toString();
    for (const Row &x : rows) sig += "|" + x.m["user_id"].toString() + ":" + x.m["name"].toString() + ":" + QString::number(x.rank) + QString::number(x.m["verified"].toBool()) + QString::number(x.m["can_kick"].toBool()) + x.m["avatar_path"].toString();
    if (sig == sig_) return;
    sig_ = sig;
    clear();
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
        const QString id = x.m["user_id"].toString(), name = x.m["name"].toString();
        QPixmap pic = profilePixmap(x.m["avatar_path"].toString(), id, name, 22, devicePixelRatioF(), pal);
        const bool verified = x.m["verified"].toBool();
        if (verified) { /* a small shield in the corner of the picture */
            QPainter pp(&pic);
            pp.drawPixmap(QPointF(0, 0), shieldPixmap(0, 11, devicePixelRatioF()));
        }
        auto *it = new QListWidgetItem(QIcon(pic), name);
        QString tip = id;
        if (verified) tip += "\nVerified";
        if (!x.m["role"].toString().isEmpty()) tip += "\n" + x.m["role"].toString();
        it->setToolTip(tip);
        it->setData(Qt::UserRole, id);
        it->setData(Qt::UserRole + 1, x.banned);
        it->setData(Qt::UserRole + 2, x.m["can_kick"].toBool());
        it->setData(Qt::UserRole + 3, x.m["can_ban"].toBool());
        it->setData(Qt::UserRole + 4, verified);
        if (x.banned) it->setForeground(pal.color(QPalette::PlaceholderText));
        addItem(it);
    }
}

}
