#include "qt/packpicker.h"
#include "qt/avatar.h"
#include <QGuiApplication>
#include <QPixmap>
#include <QScreen>
#include <QVBoxLayout>

namespace vc {

static QString S(const QJsonObject &o, const char *k) { return o.value(QLatin1String(k)).toString(); }

PackPicker::PackPicker(QWidget *parent) : QFrame(parent, Qt::Popup)
{
    setFrameShape(QFrame::StyledPanel);
    setFixedSize(380, 320);
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(4, 4, 4, 4);
    pack_ = new QComboBox;
    grid_ = new QListWidget;
    grid_->setViewMode(QListView::IconMode);
    grid_->setResizeMode(QListView::Adjust);
    grid_->setMovement(QListView::Static);
    grid_->setUniformItemSizes(true);
    grid_->setSpacing(2);
    grid_->setFrameShape(QFrame::NoFrame);
    grid_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    grid_->setIconSize(QSize(80, 80));
    grid_->setGridSize(QSize(92, 92));
    empty_ = new QLabel("No stickers here. Rooms and accounts can have sticker packs (MSC2545).");
    empty_->setWordWrap(true);
    empty_->setAlignment(Qt::AlignCenter);
    empty_->setEnabled(false);
    v->addWidget(pack_);
    v->addWidget(grid_, 1);
    v->addWidget(empty_, 1);
    connect(pack_, &QComboBox::currentIndexChanged, this, [this] { fill(); });
    connect(grid_, &QListWidget::itemClicked, this, [this](QListWidgetItem *it) {
        emit picked(it->data(Qt::UserRole).toJsonValue().toObject());
        hide();
    });
    fill(); /* shows the "no stickers" note until packs arrive */
}

void PackPicker::setPacks(const QJsonArray &packs)
{
    packs_ = packs;
    const int keep = pack_->currentIndex();
    QSignalBlocker b(pack_);
    pack_->clear();
    for (const QJsonValue &v : packs) {
        const QJsonObject p = v.toObject();
        int usable = 0;
        for (const QJsonValue &e : p["emotes"].toArray()) if (e.toObject()["sticker"].toBool()) usable++;
        if (usable == 0) continue;
        pack_->addItem(S(p, "name"), v);
    }
    if (keep >= 0 && keep < pack_->count()) pack_->setCurrentIndex(keep);
    pack_->setVisible(pack_->count() > 1);
    fill();
}

void PackPicker::fill()
{
    grid_->clear();
    const QJsonObject pack = pack_->currentData().toJsonValue().toObject();
    const QPalette pal = palette();
    const int size = 80;
    for (const QJsonValue &v : pack["emotes"].toArray()) {
        const QJsonObject e = v.toObject();
        if (!e["sticker"].toBool()) continue;
        QPixmap pm;
        if (!S(e, "path").isEmpty() && pm.load(S(e, "path"))) pm = pm.scaled(QSize(size, size) * devicePixelRatioF(), Qt::KeepAspectRatio, Qt::SmoothTransformation);
        else pm = profilePixmap(QString(), S(e, "mxc"), S(e, "shortcode"), size, devicePixelRatioF(), pal);
        pm.setDevicePixelRatio(devicePixelRatioF());
        auto *it = new QListWidgetItem(QIcon(pm), QString());
        it->setToolTip(":" + S(e, "shortcode") + ":");
        it->setData(Qt::UserRole, e);
        grid_->addItem(it);
    }
    const bool any = grid_->count() > 0;
    grid_->setVisible(any);
    empty_->setVisible(!any);
}

void PackPicker::popupAt(const QPoint &anchor)
{
    QScreen *scr = QGuiApplication::screenAt(anchor);
    if (!scr) scr = QGuiApplication::primaryScreen();
    const QRect area = scr ? scr->availableGeometry() : QRect(0, 0, 1920, 1080);
    int x = anchor.x() - width(), y = anchor.y() - height() - 4;
    if (y < area.top()) y = qMin(anchor.y() + 28, area.bottom() - height());
    move(qBound(area.left(), x, area.right() - width()), qBound(area.top(), y, area.bottom() - height()));
    show();
}

}
