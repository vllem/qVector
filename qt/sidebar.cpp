#include "qt/sidebar.h"
#include "qt/avatar.h"
#include <QJsonObject>
#include "qt/theme.h"
#include <QApplication>
#include <QElapsedTimer>
#include <QHeaderView>
#include <QMenu>
#include <QPainter>
#include <QScrollBar>
#include <QStyledItemDelegate>
#include <algorithm>

namespace vc {

enum { RoleRoom = Qt::UserRole + 1, RoleSection, RoleUnread, RoleHighlight, RoleInvite, RoleSpace };

namespace {
class Delegate : public QStyledItemDelegate {
public:
    using QStyledItemDelegate::QStyledItemDelegate;
    QSize sizeHint(const QStyleOptionViewItem &o, const QModelIndex &i) const override
    {
        QSize s = QStyledItemDelegate::sizeHint(o, i);
        s.setHeight(i.data(RoleSection).isValid() || i.data(RoleRoom).isValid() ? 24 : 28);
        return s;
    }
    void paint(QPainter *p, const QStyleOptionViewItem &o, const QModelIndex &i) const override
    {
        QStyleOptionViewItem opt = o;
        opt.state &= ~QStyle::State_HasFocus;
        QStyledItemDelegate::paint(p, opt, i);
        int n = i.data(RoleUnread).toInt();
        bool invite = i.data(RoleInvite).toBool();
        if (n <= 0 && !invite) return;
        QString t = invite ? QStringLiteral("invite") : n > 99 ? QStringLiteral("99+") : QString::number(n);
        QFont f = o.font;
        f.setPointSizeF(f.pointSizeF() * 0.85);
        f.setBold(true);
        QFontMetrics fm(f);
        int w = qMax(fm.horizontalAdvance(t) + 12, 20), h = fm.height() + 2;
        QRect r(o.rect.right() - w - 6, o.rect.center().y() - h / 2, w, h);
        p->save();
        p->setRenderHint(QPainter::Antialiasing);
        p->setBrush(i.data(RoleHighlight).toBool() ? QColor(0xc0, 0x4a, 0x4a) : QColor(0x4f, 0x74, 0x9c));
        p->setPen(Qt::NoPen);
        p->drawRoundedRect(r, h / 2.0, h / 2.0);
        p->setPen(Qt::white);
        p->setFont(f);
        p->drawText(r, Qt::AlignCenter, t);
        p->restore();
    }
};

}

Sidebar::Sidebar(QWidget *parent) : QTreeWidget(parent)
{
    setHeaderHidden(true);
    setRootIsDecorated(false);
    setIndentation(0);
    setUniformRowHeights(false);
    setFrameShape(QFrame::NoFrame);
    setItemDelegate(new Delegate(this));
    setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    setTextElideMode(Qt::ElideRight);
    setMinimumWidth(170);
    setIconSize(QSize(18, 18));
    setContextMenuPolicy(Qt::CustomContextMenu);
    connect(this, &QWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        QTreeWidgetItem *it = itemAt(pos);
        if (!it) return;
        const QString room = it->data(0, RoleRoom).toString();
        if (room.isEmpty()) return;
        QMenu menu(this);
        const auto info = rows_.constFind(room);
        if (info != rows_.constEnd() && !info->invite) {
            const bool fav = info->favourite, low = info->lowpriority;
            menu.addAction(fav ? "Remove from favourites" : "Favourite", this, [this, room, fav] { emit tagRequested(room, fav ? "none" : "favourite"); });
            menu.addAction(low ? "Remove from low priority" : "Low priority", this, [this, room, low] { emit tagRequested(room, low ? "none" : "low_priority"); });
            menu.addSeparator();
        }
        menu.addAction("Leave room", this, [this, room] { emit leaveRequested(room); });
        menu.exec(viewport()->mapToGlobal(pos));
    });
    connect(this, &QTreeWidget::itemClicked, this, [this](QTreeWidgetItem *it) {
        const QString room = it->data(0, RoleRoom).toString(), sec = it->data(0, RoleSection).toString();
        if (!room.isEmpty()) emit roomActivated(room);
        else if (!sec.isEmpty()) {
            if (collapsed_.contains(sec)) collapsed_.remove(sec); else collapsed_.insert(sec);
            sig_.clear();
            refresh(rooms_, current_, workspace_);
        }
    });
}

/* A theme switch: the rows carry brushes made from the old palette, so rebuild them once the widget's palette has really changed. */
void Sidebar::changeEvent(QEvent *e)
{
    QTreeWidget::changeEvent(e);
    if (e->type() != QEvent::PaletteChange) return;
    const QColor w = palette().color(QPalette::Window), t = palette().color(QPalette::Text);
    const QRgb key = w.rgb() ^ (t.rgb() * 31u);
    if (key == paletteKey_) return;
    paletteKey_ = key;
    sig_.clear();
    refresh(rooms_, current_, workspace_);
}

void Sidebar::refresh(const QJsonArray &rooms, const QString &current, const QString &workspace)
{
    rooms_ = rooms;
    QString sig = workspace + "|" + current + "|";
    for (const QString &c : collapsed_) sig += "c:" + c + ";";
    for (const QJsonValue &v : rooms) {
        const QJsonObject r = v.toObject();
        sig += r["id"].toString() + ":" + r["title"].toString() + ":" + r["section"].toString() + ":" + QString::number(r["unread"].toInt()) + ":" + QString::number(r["highlight"].toBool()) + ";";
    }
    if (sig == sig_) return;
    sig_ = sig;
    current_ = current;
    workspace_ = workspace;
    rows_.clear();
    for (const QJsonValue &v : rooms) {
        const QJsonObject r = v.toObject();
        rows_.insert(r["id"].toString(), {r["favourite"].toBool(), r["low_priority"].toBool(), r["invite"].toBool()});
    }

    const int scroll = verticalScrollBar()->value();
    clear();
    const QPalette pal = palette();
    const QBrush headBg(pal.color(QPalette::Window).darker(isDark(pal) ? 125 : 108));
    const qreal dpr = devicePixelRatioF();

    QString lastSection;
    bool folded = false;
    for (const QJsonValue &v : rooms) {
        const QJsonObject r = v.toObject();
        const QString section = r["section"].toString();
        if (section != lastSection) {
            lastSection = section;
            folded = collapsed_.contains(section);
            int unread = 0, highlight = 0;
            for (const QJsonValue &w : rooms) { const QJsonObject o = w.toObject(); if (o["section"].toString() == section) { unread += o["unread"].toInt(); highlight += o["highlight"].toBool(); } }
            auto *h = new QTreeWidgetItem(this, QStringList((folded ? QStringLiteral("\u25B8  ") : QStringLiteral("\u25BE  ")) + section));
            h->setData(0, RoleSection, section);
            h->setFlags(Qt::ItemIsEnabled);
            h->setBackground(0, headBg);
            if (folded && unread > 0) { h->setData(0, RoleUnread, unread); h->setData(0, RoleHighlight, highlight > 0); }
        }
        if (folded) continue;
        const QString title = r["title"].toString(), id = r["id"].toString();
        const bool dm = section == "Direct Messages", invite = r["invite"].toBool();
        const int unread = r["unread"].toInt();
        auto *it = new QTreeWidgetItem(this, QStringList((dm || invite) ? title : "#" + title));
        it->setData(0, RoleRoom, id);
        it->setData(0, RoleUnread, unread);
        it->setData(0, RoleHighlight, r["highlight"].toBool());
        it->setData(0, RoleInvite, invite);
        it->setIcon(0, QIcon(profilePixmap(r["avatar_path"].toString(), id, title, 18, dpr, pal))); /* every room has a picture: its own avatar, or its initial on a colour */
        if (unread == 0 && !invite) it->setForeground(0, pal.color(QPalette::PlaceholderText));
        if (id == current) it->setSelected(true);
    }
    verticalScrollBar()->setValue(scroll);
}

}
