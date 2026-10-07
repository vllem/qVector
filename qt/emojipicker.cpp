#include "qt/emojipicker.h"
#include "qt/theme.h"
#include <QFontDatabase>
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
    setCategory(recentEmoji().isEmpty() ? 1 : 0);
    show();
    search_->setFocus();
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
    grid_->setIconSize(QSize());
    grid_->setGridSize(QSize(42, 42));
    QList<Emoji> shown;
    const QString q = search_->text().trimmed();
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
