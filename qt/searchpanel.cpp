#include "qt/searchpanel.h"
#include "qt/theme.h"
#include <QDateTime>
#include <QJsonObject>
#include <QHBoxLayout>
#include <QShortcut>
#include <QToolButton>
#include <QVBoxLayout>

namespace vc {

SearchPanel::SearchPanel(QWidget *parent) : QWidget(parent)
{
    auto *v = new QVBoxLayout(this);
    v->setContentsMargins(0, 0, 0, 0);
    v->setSpacing(0);
    auto *head = new QHBoxLayout;
    head->setContentsMargins(8, 4, 4, 4);
    auto *title = new QLabel("Search");
    QFont f = title->font();
    f.setBold(true);
    title->setFont(f);
    scope_ = new QComboBox;
    scope_->addItems({"This room", "All rooms"});
    auto *close = new QToolButton;
    close->setText(QStringLiteral("✕"));
    close->setAutoRaise(true);
    close->setToolTip("Close the search (Esc)");
    head->addWidget(title);
    head->addStretch(1);
    head->addWidget(scope_);
    head->addWidget(close);
    v->addLayout(head);
    input_ = new QLineEdit;
    input_->setPlaceholderText("Search messages");
    input_->setClearButtonEnabled(true);
    input_->setContentsMargins(4, 2, 4, 2);
    v->addWidget(input_);
    list_ = new QListWidget;
    list_->setFrameShape(QFrame::NoFrame);
    list_->setWordWrap(true);
    list_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    v->addWidget(list_, 1);
    status_ = new QLabel;
    status_->setContentsMargins(8, 3, 8, 3);
    status_->setWordWrap(true);
    v->addWidget(status_);
    setMinimumWidth(280);
    (void)new QShortcut(Qt::Key_Escape, this, [this] { emit closeRequested(); }, Qt::WidgetWithChildrenShortcut);
    debounce_.setSingleShot(true);
    connect(&debounce_, &QTimer::timeout, this, [this] { search(); });
    connect(input_, &QLineEdit::textChanged, this, [this] { debounce_.start(350); });
    connect(input_, &QLineEdit::returnPressed, this, [this] { debounce_.stop(); search(); });
    connect(scope_, &QComboBox::currentIndexChanged, this, [this] { search(); });
    connect(close, &QToolButton::clicked, this, &SearchPanel::closeRequested);
    connect(list_, &QListWidget::itemActivated, this, [this](QListWidgetItem *it) {
        emit jumpRequested(it->data(Qt::UserRole).toString(), it->data(Qt::UserRole + 1).toString());
    });
    connect(list_, &QListWidget::itemClicked, this, [this](QListWidgetItem *it) {
        emit jumpRequested(it->data(Qt::UserRole).toString(), it->data(Qt::UserRole + 1).toString());
    });
}

void SearchPanel::focusInput()
{
    input_->setFocus();
    input_->selectAll();
}

void SearchPanel::search()
{
    const QString term = input_->text().trimmed();
    if (term.isEmpty()) { list_->clear(); status_->clear(); return; }
    const bool all = scope_->currentIndex() == 1;
    if (all && !indexOn_) { list_->clear(); status_->setText("Searching every room needs \"Search all my messages\" in Preferences (a local, encrypted index)."); return; }
    status_->setText("Searching...");
    emit searchRequested(term, all);
}

void SearchPanel::setResults(const QJsonArray &hits)
{
    list_->clear();
    for (const QJsonValue &v : hits) {
        const QJsonObject h = v.toObject();
        const QString room = h["room"].toString();
        auto *it = new QListWidgetItem(QString("%1  -  %2%3\n%4").arg(h["sender"].toString(), h["time"].toString(), room.isEmpty() ? QString() : "  -  #" + room, h["body"].toString().left(160)));
        it->setData(Qt::UserRole, h["room_id"].toString().isEmpty() ? room_ : h["room_id"].toString());
        it->setData(Qt::UserRole + 1, h["id"].toString());
        list_->addItem(it);
    }
    status_->setText(hits.isEmpty() ? "No messages found (only loaded messages are searched; turn on the message index in Preferences to search everything)" : QString("%1 found").arg(hits.size()));
}

}
