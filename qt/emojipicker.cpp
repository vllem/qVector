#include "qt/emojipicker.h"
#include "qt/theme.h"
#include <QFontDatabase>
#include <QPixmap>
#include <QGuiApplication>
#include <QScreen>
#include <QHBoxLayout>
#include <QMenu>
#include <QVBoxLayout>

namespace vc {

EmojiPicker::EmojiPicker(QWidget *parent) : QFrame(parent, Qt::Popup)
{
    setFrameShape(QFrame::StyledPanel);
    setFixedSize(400, 340);
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(4, 4, 4, 4);
    search_ = new QLineEdit;
    search_->setPlaceholderText("Search emoji");
    search_->setClearButtonEnabled(true);
    {   /* one flat button per category in a single row: no scrolling tab strip, nothing cut off or misaligned */
        auto *row = new QHBoxLayout;
        row->setContentsMargins(0, 0, 0, 0);
        row->setSpacing(2);
        const QStringList groups = emojiGroups();
        static const char *const ICONS[] = {"\xF0\x9F\x98\x80", "\xF0\x9F\x91\x8B", "\xF0\x9F\x90\xB6", "\xF0\x9F\x8D\x8E", "\xE2\x9C\x88\xEF\xB8\x8F", "\xE2\x9A\xBD",
                                            "\xF0\x9F\x92\xA1", "\xE2\x9D\xA4\xEF\xB8\x8F", "\xF0\x9F\x8F\x81"};
        QStringList labels{QStringLiteral("\u2605"), QString(), QString(), QString(), QString(), QString(), QString(), QString(), QString(), QString()};
        QStringList tips{"Recently used"};
        for (int i = 0; i < groups.size() && i < 9; i++) { labels[i + 1] = QString::fromUtf8(i < 9 ? ICONS[i] : "?"); tips << groups[i]; }
        QFont bf = font();
        bf.setPointSize(13);
        if (!emojiFamily().isEmpty()) bf.setFamilies({emojiFamily(), bf.family()});
        for (int i = 0; i < labels.size(); i++) {
            auto *b = new QToolButton;
            b->setText(labels[i]);
            b->setToolTip(tips[i]);
            b->setCheckable(true);
            b->setAutoRaise(true);
            b->setFont(i == 0 ? font() : bf);
            b->setFixedHeight(32);
            b->setMinimumWidth(30);
            b->setToolButtonStyle(Qt::ToolButtonTextOnly);
            cats_ << b;
            row->addWidget(b);
            connect(b, &QToolButton::clicked, this, [this, i] { setCategory(i); });
        }
        customBtn_ = new QToolButton; /* shows the first custom emoji; hidden when the room has none */
        customBtn_->setToolTip("Custom emoji");
        customBtn_->setCheckable(true);
        customBtn_->setAutoRaise(true);
        customBtn_->setFixedHeight(32);
        customBtn_->setMinimumWidth(30);
        customBtn_->setIconSize(QSize(22, 22));
        customBtn_->hide();
        cats_ << customBtn_;
        row->addWidget(customBtn_);
        connect(customBtn_, &QToolButton::clicked, this, [this] { setCategory(10); });
        row->addStretch(1);
        v->addWidget(search_);
        v->addLayout(row);
    }
    grid_ = new QListWidget;
    grid_->setViewMode(QListView::IconMode);
    grid_->setResizeMode(QListView::Adjust);
    grid_->setMovement(QListView::Static);
    grid_->setUniformItemSizes(true);
    grid_->setSpacing(2);
    grid_->setFrameShape(QFrame::NoFrame);
    grid_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    v->addWidget(grid_, 1);
    connect(search_, &QLineEdit::textChanged, this, [this] { fill(); });
    connect(grid_, &QListWidget::itemClicked, this, [this](QListWidgetItem *it) {
        const QString g = it->data(Qt::UserRole).toString();
        if (it->data(Qt::UserRole + 1).toBool()) { emit customPicked(g); if (closeOnPick_) hide(); return; } /* a custom emoji has no glyph to remember */
        rememberEmoji(g);
        emit emojiPicked(g);
        if (closeOnPick_) hide();
    });
}

/* anchor: the top-right corner of whatever opened us. The popup sits above it, right-aligned, and is kept on the screen. */
void EmojiPicker::popupAt(const QPoint &anchor)
{
    QScreen *scr = QGuiApplication::screenAt(anchor);
    if (!scr) scr = QGuiApplication::primaryScreen();
    const QRect area = scr ? scr->availableGeometry() : QRect(0, 0, 1920, 1080);
    int x = anchor.x() - width(), y = anchor.y() - height() - 4;
    if (y < area.top()) y = qMin(anchor.y() + 28, area.bottom() - height()); /* no room above: below it */
    x = qBound(area.left(), x, area.right() - width());
    y = qBound(area.top(), y, area.bottom() - height());
    move(x, y);
    search_->clear();
    setCategory(cat_ == 10 && !custom_.isEmpty() ? 10 : recentEmoji().isEmpty() ? 1 : 0);
    show();
    search_->setFocus();
}

void EmojiPicker::setCustom(const QJsonArray &packs)
{
    custom_.clear();
    for (const QJsonValue &p : packs)
        for (const QJsonValue &e : p.toObject()["emotes"].toArray()) if (e.toObject()["emoji"].toBool()) custom_ << e.toObject();
    customBtn_->setVisible(!custom_.isEmpty());
    QIcon icon;
    for (const QJsonObject &e : custom_) { QPixmap pm; if (pm.load(e["path"].toString())) { icon = QIcon(pm); break; } }
    customBtn_->setIcon(icon);
    customBtn_->setText(icon.isNull() ? "+" : QString());
    customBtn_->setToolButtonStyle(icon.isNull() ? Qt::ToolButtonTextOnly : Qt::ToolButtonIconOnly);
    if (custom_.isEmpty() && cat_ == 10) cat_ = 1;
    if (isVisible()) fill();
}

void EmojiPicker::setCategory(int i)
{
    cat_ = i;
    for (int k = 0; k < cats_.size(); k++) cats_[k]->setChecked(k == i);
    fill();
}

void EmojiPicker::fill()
{
    grid_->clear();
    QFont f = grid_->font();
    f.setPointSize(16);
    if (!emojiFamily().isEmpty()) f.setFamilies({emojiFamily(), f.family()}); /* an emoji font first, so every glyph comes out in colour */
    grid_->setFont(f);
    grid_->setTextElideMode(Qt::ElideNone);
    grid_->setGridSize(QSize(42, 42));
    QList<Emoji> shown;
    const QString q = search_->text().trimmed();
    auto addCustom = [&](const QJsonObject &e) {
        QPixmap pm;
        if (!pm.load(e["path"].toString())) return;
        pm = pm.scaled(QSize(28, 28) * devicePixelRatioF(), Qt::KeepAspectRatio, Qt::SmoothTransformation);
        pm.setDevicePixelRatio(devicePixelRatioF());
        auto *it = new QListWidgetItem(QIcon(pm), QString());
        it->setToolTip(":" + e["shortcode"].toString() + ":");
        it->setData(Qt::UserRole, e["shortcode"].toString());
        it->setData(Qt::UserRole + 1, true);
        grid_->addItem(it);
    };
    if (cat_ == 10 && q.isEmpty()) { grid_->setIconSize(QSize(28, 28)); for (const QJsonObject &e : custom_) addCustom(e); return; }
    grid_->setIconSize(QSize()); /* glyphs are drawn as text, nothing is reserved for an icon */
    if (!q.isEmpty()) shown = searchEmoji(q, 200);
    else if (cat_ == 0) {
        const QStringList rec = recentEmoji();
        for (const QString &g : rec) shown.append({g, g, 0});
    } else {
        for (const Emoji &e : allEmoji()) if (e.group == cat_ - 1) shown.append(e);
    }
    for (const Emoji &e : shown) {
        auto *it = new QListWidgetItem(e.glyph);
        it->setToolTip(e.name);
        it->setData(Qt::UserRole, e.glyph);
        it->setTextAlignment(Qt::AlignCenter);
        grid_->addItem(it);
    }
}

}
