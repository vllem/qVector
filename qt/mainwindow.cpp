#include "qt/mainwindow.h"
#include "qt/audioplayer.h"
#include "qt/avatar.h"
#include "qt/theme.h"
#include <QApplication>
#include <QClipboard>
#include <QDesktopServices>
#include <QDragEnterEvent>
#include <QDropEvent>
#include <QFileInfo>
#include <QLocale>
#include <QPixmap>
#include <QUrl>
#include <QDir>
#include <QFileDialog>
#include <QFrame>
#include <QInputDialog>
#include <QComboBox>
#include <QCheckBox>
#include <QGroupBox>
#include <QFormLayout>
#include <QDateTime>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>
#include <QMimeData>
#include <QSettings>
#include <QStatusBar>
#include <QTimer>
#include <QVBoxLayout>

#include <QProxyStyle>
#include <QStyleOptionTab>
#include <QPainter>
#include <QPainterPath>

namespace vc {

namespace {
/* Flat rectangular tabs drawn on top of the platform style, so the close button, hover and drag behaviour stay native
   (a style sheet would replace all of that). The selected tab is filled with the accent colour. */
class FlatTabStyle : public QProxyStyle {
public:
    explicit FlatTabStyle(const QString &base) : QProxyStyle(base) {}
    void setLine(const QColor &c) { line_ = c; }
    void drawControl(ControlElement el, const QStyleOption *opt, QPainter *p, const QWidget *w) const override
    {
        const auto *tab = qstyleoption_cast<const QStyleOptionTab *>(opt);
        if (tab && el == CE_TabBarTabShape) {
            const bool sel = tab->state & State_Selected, hover = tab->state & State_MouseOver;
            const QPalette &pal = tab->palette;
            p->fillRect(tab->rect, sel ? pal.color(QPalette::Highlight) : hover ? pal.color(QPalette::Midlight) : pal.color(QPalette::Window));
            if (!sel && tab->selectedPosition != QStyleOptionTab::NextIsSelected) { /* inset separator between tabs */
                p->fillRect(QRect(tab->rect.right(), tab->rect.top() + 6, 1, tab->rect.height() - 12), line_); /* same grey as the splitter */
            }
            return;
        }
        if (tab && el == CE_TabBarTabLabel && (tab->state & State_Selected)) {
            QStyleOptionTab o(*tab);
            o.palette.setColor(QPalette::WindowText, onHighlight(tab->palette));
            o.palette.setColor(QPalette::ButtonText, onHighlight(tab->palette));
            QProxyStyle::drawControl(el, &o, p, w);
            return;
        }
        QProxyStyle::drawControl(el, opt, p, w);
    }
private:
    QColor line_ = QColor(Qt::gray);
};
}

Composer::Composer(QWidget *parent) : QLineEdit(parent)
{
    popup_ = new QListWidget(nullptr);
    popup_->setWindowFlags(Qt::ToolTip | Qt::FramelessWindowHint);
    popup_->setAttribute(Qt::WA_ShowWithoutActivating);
    popup_->setFocusPolicy(Qt::NoFocus);
    popup_->setFrameShape(QFrame::StyledPanel);
    popup_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    connect(this, &QLineEdit::textEdited, this, [this] { updatePopup(); });
    connect(this, &QLineEdit::cursorPositionChanged, this, [this] { if (popup_->isVisible()) updatePopup(); });
    connect(popup_, &QListWidget::itemClicked, this, [this] { acceptCandidate(); });
}

Composer::~Composer() { delete popup_; }

void Composer::setButtons(QToolButton *left, QToolButton *right)
{
    left_ = left;
    right_ = right;
    for (QToolButton *b : {left, right}) b->setIconSize(QSize(20, 20));
    setTextMargins(28, 0, 28, 0);
}

void Composer::resizeEvent(QResizeEvent *e)
{
    QLineEdit::resizeEvent(e);
    if (!left_) return;
    left_->setFixedSize(28, 28);
    right_->setFixedSize(28, 28);
    left_->move(5, (height() - 28) / 2);
    right_->move(width() - 33, (height() - 28) / 2);
}

void Composer::focusOutEvent(QFocusEvent *e)
{
    popup_->hide();
    QLineEdit::focusOutEvent(e);
}

/* The word being typed, when it starts with '@': offer the matching people. */
void Composer::updatePopup()
{
    const QString t = text();
    int pos = cursorPosition(), start = pos;
    while (start > 0 && !t[start - 1].isSpace()) start--;
    if (start >= pos || (t[start] != '@' && t[start] != ':')) { popup_->hide(); tokenStart_ = -1; return; }
    if (t[start] == ':') { /* :smile -> emoji by name */
        const QString word = t.mid(start + 1, pos - start - 1);
        shown_.clear();
        if (word.size() >= 2) for (const Emoji &e : searchEmoji(word, 8)) shown_.append({QString(), e.glyph + "  " + e.name, e.glyph + " "});
    } else {
        if (!candidates) { popup_->hide(); tokenStart_ = -1; return; }
        shown_ = candidates(t.mid(start + 1, pos - start - 1));
    }
    if (shown_.isEmpty()) { popup_->hide(); tokenStart_ = -1; return; }
    tokenStart_ = start;
    popup_->clear();
    for (const Candidate &c : shown_) popup_->addItem(c.id.isEmpty() ? c.label : c.label + "  (" + c.id + ")");
    popup_->setCurrentRow(0);
    const int rows = qMin(int(shown_.size()), 8);
    popup_->resize(qMax(260, width() / 2), rows * popup_->sizeHintForRow(0) + 8);
    popup_->move(mapToGlobal(QPoint(0, -popup_->height() - 2)));
    popup_->show();
}

void Composer::acceptCandidate()
{
    const int row = popup_->currentRow();
    if (row < 0 || row >= shown_.size() || tokenStart_ < 0) return;
    const Candidate c = shown_[row];
    QString t = text();
    const int end = cursorPosition();
    const QString ins = c.insert.isEmpty() ? c.label + " " : c.insert;
    t.replace(tokenStart_, end - tokenStart_, ins);
    setText(t);
    setCursorPosition(tokenStart_ + ins.size());
    popup_->hide();
    tokenStart_ = -1;
    if (!c.id.isEmpty()) emit mentionPicked(c.id, c.label);
    emit textEdited(text());
}

void Composer::keyPressEvent(QKeyEvent *e)
{
    if (popup_->isVisible()) {
        switch (e->key()) {
        case Qt::Key_Down: popup_->setCurrentRow(qMin(popup_->currentRow() + 1, popup_->count() - 1)); return;
        case Qt::Key_Up: popup_->setCurrentRow(qMax(popup_->currentRow() - 1, 0)); return;
        case Qt::Key_Return: case Qt::Key_Enter: {
            /* the whole name is already typed: take it as the mention and send, instead of needing a space first */
            const QString typed = text().mid(tokenStart_ >= 0 ? tokenStart_ : 0, cursorPosition() - qMax(tokenStart_, 0));
            for (int i = 0; i < shown_.size(); i++)
                if (!shown_[i].id.isEmpty() && shown_[i].label.compare(typed, Qt::CaseInsensitive) == 0) {
                    popup_->setCurrentRow(i);
                    acceptCandidate();
                    QLineEdit::keyPressEvent(e);
                    return;
                }
            acceptCandidate();
            return;
        }
        case Qt::Key_Tab: acceptCandidate(); return;
        case Qt::Key_Escape: popup_->hide(); return;
        default: break;
        }
    }
    if (e->matches(QKeySequence::Paste)) {
        const QMimeData *md = QApplication::clipboard()->mimeData();
        if (md && md->hasUrls()) {
            QStringList files;
            for (const QUrl &u : md->urls()) if (u.isLocalFile() && QFileInfo(u.toLocalFile()).isFile()) files << u.toLocalFile();
            if (!files.isEmpty()) { emit filesPasted(files); return; }
        }
        if (md && md->hasImage() && !md->hasText()) {
            QImage img = qvariant_cast<QImage>(md->imageData());
            if (!img.isNull()) { emit imagePasted(img); return; }
        }
        if (md && md->hasImage()) {
            QImage img = qvariant_cast<QImage>(md->imageData());
            if (!img.isNull() && text().isEmpty()) { emit imagePasted(img); return; }
        }
    }
    if (e->key() == Qt::Key_Escape) { emit escapePressed(); return; }
    QLineEdit::keyPressEvent(e);
}


static QString S(const QJsonObject &o, const char *k) { return o.value(QLatin1String(k)).toString(); }

MainWindow::MainWindow(Core *core, bool demo) : core_(core), demo_(demo)
{
    setWindowTitle("qVector");
    resize(1100, 740);
    notify_ = QSettings("vector", "vector").value("notifications", true).toBool();
    notifier_ = new Notifier(this);
    connect(notifier_, &Notifier::activated, this, [this](const QString &id) { showNormal(); raise(); activateWindow(); openRoom(id); });
    setWindowIcon(appIcon());
    if (QSystemTrayIcon::isSystemTrayAvailable()) {
        tray_ = new QSystemTrayIcon(this);
        tray_->setIcon(QIcon(trayPixmap(0, 64)));
        tray_->setToolTip("qVector");
        auto *tm = new QMenu(this);
        tm->addAction("Show qVector", this, [this] { showNormal(); raise(); activateWindow(); });
        tm->addSeparator();
        tm->addAction("Quit", this, [] { qApp->quit(); });
        tray_->setContextMenu(tm);
        connect(tray_, &QSystemTrayIcon::activated, this, [this](QSystemTrayIcon::ActivationReason why) {
            if (why != QSystemTrayIcon::Trigger) return;
            if (isVisible() && isActiveWindow()) hide(); else { showNormal(); raise(); activateWindow(); }
        });
        tray_->setVisible(QSettings("vector", "vector").value("tray", true).toBool());
        notifier_->setTray(tray_);
    }
    root_ = new QStackedWidget;
    login_ = new LoginPage(core_);
    root_->addWidget(login_);

    auto *mainPage = new QWidget;
    auto *mv = new QVBoxLayout(mainPage);
    mv->setContentsMargins(0, 0, 0, 0);
    mv->setSpacing(0);
    split_ = new QSplitter;
    split_->setChildrenCollapsible(false);
    split_->setHandleWidth(2);
    sidebar_ = new Sidebar;
    split_->addWidget(sidebar_);

    auto *right = new QWidget;
    auto *rv = new QVBoxLayout(right);
    rv->setContentsMargins(0, 0, 0, 0);
    rv->setSpacing(0);

    auto *tabRow = new QHBoxLayout;
    tabRow->setContentsMargins(0, 0, 0, 0);
    tabRow->setSpacing(0);
    tabs_ = new QTabBar;
    tabs_->setTabsClosable(true);
    tabs_->setMovable(true);
    tabs_->setExpanding(false);
    tabs_->setUsesScrollButtons(true);
    tabs_->setElideMode(Qt::ElideRight);
    tabs_->setIconSize(QSize(16, 16));
    tabs_->setDrawBase(false);
    plus_ = new QToolButton;
    plus_->setText("+");
    plus_->setAutoRaise(true);
    tabRow->addWidget(tabs_, 1);
    tabRow->addWidget(plus_);
    rv->addLayout(tabRow);
    tabLine_ = new QFrame; /* divider between the tabs and the topic / pinned message */
    tabLine_->setFixedHeight(1);
    rv->addWidget(tabLine_);

    auto *bar = new QHBoxLayout;
    bar->setContentsMargins(6, 3, 8, 3);
    topic_ = new QLabel;
    topic_->setTextFormat(Qt::PlainText);
    topic_->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    membersBtn_ = new QToolButton;
    membersBtn_->setAutoRaise(true);
    membersBtn_->setToolTip("Show or hide the member list (Ctrl+M)");
    bar->addWidget(topic_, 1);
    bar->addWidget(membersBtn_);
    rv->addLayout(bar);
    topicLine_ = new QFrame; /* divider under the topic row */
    topicLine_->setFixedHeight(1);
    rv->addWidget(topicLine_);

    banner_ = new QLabel;
    banner_->setWordWrap(true);
    banner_->hide();
    rv->addWidget(banner_);

    pinBar_ = new QWidget;
    {
        auto *pl = new QHBoxLayout(pinBar_);
        pl->setContentsMargins(8, 2, 8, 2);
        pinText_ = new QPushButton;
        pinText_->setFlat(true);
        pinText_->setStyleSheet("text-align:left;padding:2px 4px");
        pinText_->setToolTip("Show the pinned message");
        pinPrev_ = new QPushButton("\u2039");
        pinNext_ = new QPushButton("\u203A");
        pinOff_ = new QPushButton("Unpin");
        for (QPushButton *b : {pinPrev_, pinNext_}) { b->setFlat(true); b->setFixedWidth(26); }
        pl->addWidget(pinText_, 1);
        pl->addWidget(pinPrev_);
        pl->addWidget(pinNext_);
        pl->addWidget(pinOff_);
        pinBar_->hide();
        connect(pinText_, &QPushButton::clicked, this, [this] { if (pinIndex_ >= 0 && pinIndex_ < pinned_.size()) timeline_->revealMessage(pinned_[pinIndex_]["id"].toString()); });
        connect(pinPrev_, &QPushButton::clicked, this, [this] { pinIndex_--; updatePinBar(); });
        connect(pinNext_, &QPushButton::clicked, this, [this] { pinIndex_++; updatePinBar(); });
        connect(pinOff_, &QPushButton::clicked, this, [this] { if (pinIndex_ >= 0 && pinIndex_ < pinned_.size()) core_->call("pin_message", {{"event_id", pinned_[pinIndex_]["id"].toString()}, {"pinned", false}}); });
    }
    pinLine_ = new QFrame;
    pinLine_->setFixedHeight(1);
    pinLine_->hide();
    rv->addWidget(pinBar_);
    rv->addWidget(pinLine_); /* divider under the pinned message */

    inviteBar_ = new QWidget;
    {
        auto *ib = new QHBoxLayout(inviteBar_);
        ib->setContentsMargins(10, 6, 10, 6);
        auto *il = new QLabel("You have been invited to this room");
        auto *acc = new QPushButton("Accept");
        auto *dec = new QPushButton("Decline");
        ib->addWidget(il, 1);
        ib->addWidget(acc);
        ib->addWidget(dec);
        inviteBar_->setAutoFillBackground(true);
        inviteBar_->hide();
        connect(acc, &QPushButton::clicked, this, [this] { core_->call("accept_invite", {{"room_id", current_}}); });
        connect(dec, &QPushButton::clicked, this, [this] { core_->call("leave_room", {{"room_id", current_}}); });
    }
    rv->addWidget(inviteBar_);

    timelines_ = new QStackedWidget;
    empty_ = new QLabel("Select a room");
    static_cast<QLabel *>(empty_)->setAlignment(Qt::AlignCenter);
    timelines_->addWidget(empty_);
    timeline_ = new TimelineView;
    timelines_->addWidget(timeline_);
    rv->addWidget(timelines_, 1);

    typingLabel_ = new QLabel;
    typingLabel_->setContentsMargins(8, 1, 8, 1);
    {
        QFont tf = typingLabel_->font();
        tf.setItalic(true);
        typingLabel_->setFont(tf);
        QPalette tp = typingLabel_->palette();
        tp.setColor(QPalette::WindowText, tp.color(QPalette::PlaceholderText));
        typingLabel_->setPalette(tp);
    }
    typingLabel_->setFixedHeight(typingLabel_->fontMetrics().height() + 4);
    rv->addWidget(typingLabel_);
    auto *compRow = new QHBoxLayout;
    compRow->setContentsMargins(8, 6, 8, 8);
    compRow->setSpacing(4);
    composer_ = new Composer;
    composer_->setPlaceholderText("Send a message");
    composer_->setMinimumHeight(48);

    /* Attach on the left and emoji on the right, both inside the box; the icons are drawn in applyChrome in the palette's text colour. */
    attach_ = new QToolButton(composer_); /* real tool buttons: the style draws hover and pressed feedback */
    attach_->setAutoRaise(true);
    attach_->setCursor(Qt::ArrowCursor);
    attach_->setToolTip("Send a file or image");
    emojiBtn_ = new QToolButton(composer_);
    emojiBtn_->setAutoRaise(true);
    emojiBtn_->setCursor(Qt::ArrowCursor);
    emojiBtn_->setToolTip("Emoji (you can also type :name)");
    composer_->setButtons(attach_, emojiBtn_);
    compRow->addWidget(composer_, 1);
    contextBar_ = new QWidget;
    {
        auto *cl = new QHBoxLayout(contextBar_);
        cl->setContentsMargins(8, 3, 6, 3);
        contextLabel_ = new QLabel;
        auto *cx = new QToolButton;
        cx->setText(QStringLiteral("\u2715"));
        cx->setAutoRaise(true);
        cx->setToolTip("Cancel (Esc)");
        cl->addWidget(contextLabel_, 1);
        cl->addWidget(cx);
        connect(cx, &QToolButton::clicked, this, [this] { cancelContext(); });
        contextBar_->setAutoFillBackground(true);
        contextBar_->hide();
    }
    rv->addWidget(contextBar_);
    attachBar_ = new QWidget;
    {
        auto *al = new QHBoxLayout(attachBar_);
        al->setContentsMargins(8, 3, 6, 3);
        attachThumb_ = new QLabel;
        attachLabel_ = new QLabel;
        auto *ax = new QToolButton;
        ax->setText(QStringLiteral("\u2715"));
        ax->setAutoRaise(true);
        ax->setToolTip("Do not send (Esc)");
        al->addWidget(attachThumb_);
        al->addWidget(attachLabel_, 1);
        al->addWidget(ax);
        connect(ax, &QToolButton::clicked, this, [this] { cancelAttachment(); composer_->setFocus(); });
        attachBar_->setAutoFillBackground(true);
        attachBar_->hide();
    }
    rv->addWidget(attachBar_);
    setAcceptDrops(true);
    /* The text boxes take dropped files as text (or refuse them), so the drop never reached the window for some file types: let them pass it on. */
    composer_->setAcceptDrops(false);
    for (QTextEdit *t : right->findChildren<QTextEdit *>()) { t->setAcceptDrops(false); t->viewport()->setAcceptDrops(false); }
    rv->addLayout(compRow);
    split_->addWidget(right);
    memberList_ = new MemberList;
    memberList_->hide();
    split_->addWidget(memberList_);
    threadPanel_ = new QWidget;
    {
        auto *tl = new QVBoxLayout(threadPanel_);
        tl->setContentsMargins(0, 0, 0, 0);
        tl->setSpacing(0);
        auto *head = new QHBoxLayout;
        head->setContentsMargins(8, 4, 4, 4);
        threadTitle_ = new QLabel("Thread");
        QFont tf = threadTitle_->font();
        tf.setBold(true);
        threadTitle_->setFont(tf);
        auto *close = new QToolButton;
        close->setText(QStringLiteral("\u2715"));
        close->setAutoRaise(true);
        close->setToolTip("Close the thread");
        head->addWidget(threadTitle_, 1);
        head->addWidget(close);
        tl->addLayout(head);
        threadView_ = new TimelineView(true);
        tl->addWidget(threadView_, 1);
        threadInput_ = new QLineEdit;
        threadInput_->setPlaceholderText("Reply in thread");
        threadInput_->setFrame(false);
        threadInput_->setContentsMargins(6, 4, 6, 4);
        tl->addWidget(threadInput_);
        threadPanel_->setMinimumWidth(260);
        threadPanel_->hide();
        connect(close, &QToolButton::clicked, this, [this] { closeThread(); });
        connect(threadInput_, &QLineEdit::returnPressed, this, [this] { sendThreadReply(); });
    }
    split_->addWidget(threadPanel_);
    searchPanel_ = new SearchPanel;
    searchPanel_->hide();
    split_->addWidget(searchPanel_);
    split_->setStretchFactor(0, 0);
    split_->setStretchFactor(1, 1);
    split_->setStretchFactor(2, 0);
    split_->setStretchFactor(3, 0);
    split_->setStretchFactor(4, 0);
    split_->setSizes({210, 890, 0, 0, 0});
    connect(searchPanel_, &SearchPanel::closeRequested, this, [this] { searchPanel_->hide(); layoutPanes(); composer_->setFocus(); });
    connect(searchPanel_, &SearchPanel::jumpRequested, this, &MainWindow::jumpTo);
    connect(searchPanel_, &SearchPanel::searchRequested, this, [this](const QString &term, bool all) { core_->call(all ? "search_all" : "search_messages", {{"query", term}}); });
    connect(membersBtn_, &QToolButton::clicked, this, &MainWindow::toggleMembers);
    connect(memberList_, &MemberList::verifyRequested, this, &MainWindow::verifyPerson);
    connect(memberList_, &MemberList::messageRequested, this, [this](const QString &id) { core_->call("start_dm", {{"user_id", id}}); });
    connect(memberList_, &MemberList::moderationRequested, this, [this](const QString &action, const QString &user) {
        if (action == "kick" || action == "ban") {
            bool ok = false;
            const QString reason = QInputDialog::getText(this, action == "kick" ? "Kick " + user : "Ban " + user, "Reason (optional)", QLineEdit::Normal, QString(), &ok);
            if (ok) core_->call("room_action", {{"kind", action}, {"a", user}, {"b", reason}});
        } else if (action == "unban") core_->call("room_action", {{"kind", "unban"}, {"a", user}, {"b", ""}});
        else if (action.startsWith("role:")) core_->call("room_action", {{"kind", action.mid(5)}, {"a", user}, {"b", ""}});
    });
    connect(threadView_, &TimelineView::reactRequested, this, [this](const QString &id, const QString &key) { core_->call("react", {{"event_id", id}, {"key", key}}); });
    connect(threadView_, &TimelineView::deleteRequested, this, [this](const QString &id) { core_->call("redact", {{"event_id", id}}); });
    mv->addWidget(split_, 1);
    statusLine_ = new QFrame; /* the same divider as between the room list and the chats, above the status bar */
    statusLine_->setFixedHeight(2);
    mv->addWidget(statusLine_);
    root_->addWidget(mainPage);
    setCentralWidget(root_);

    statusLeft_ = new QLabel;
    statusRight_ = new QLabel;
    statusBar()->setSizeGripEnabled(false);
    statusSep1_ = new QFrame;
    statusSep2_ = new QFrame;
    for (QFrame *f : {statusSep1_, statusSep2_}) { f->setFrameShape(QFrame::NoFrame); f->setFixedSize(2, 16); }
    statusLeft_->setContentsMargins(6, 0, 8, 0);
    statusRight_->setContentsMargins(8, 0, 8, 0);
    statusBar()->addWidget(statusLeft_);
    statusBar()->addWidget(statusSep1_);
    statusBar()->addWidget(statusRight_);
    statusBar()->addWidget(statusSep2_);
    statusSep1_->hide();
    statusSep2_->hide();
    statusBar()->addWidget(new QWidget, 1);
    statusSep3_ = new QFrame;
    statusSep3_->setFrameShape(QFrame::NoFrame);
    statusSep3_->setFixedSize(2, 16);
    statusClock_ = new QLabel;
    statusClock_->setContentsMargins(8, 0, 8, 0);
    statusBar()->addPermanentWidget(statusSep3_);
    statusBar()->addPermanentWidget(statusClock_);
    auto *clockTimer = new QTimer(this);
    connect(clockTimer, &QTimer::timeout, this, &MainWindow::updateClock);
    clockTimer->start(1000);
    updateClock();
    applyChrome();
    buildMenus();

    connect(core_, &Core::event, this, &MainWindow::onEvent);
    connect(sidebar_, &Sidebar::roomActivated, this, [this](const QString &id) { openRoom(id); });
    connect(sidebar_, &Sidebar::tagRequested, this, [this](const QString &id, const QString &kind) { core_->call("set_room_tag", {{"room_id", id}, {"kind", kind}}); });
    connect(sidebar_, &Sidebar::leaveRequested, this, [this](const QString &id) {
        if (QMessageBox::question(this, "Leave", "Leave " + roomTitle(id) + "?") != QMessageBox::Yes) return;
        core_->call("leave_room", {{"room_id", id}});
    });
    connect(tabs_, &QTabBar::currentChanged, this, [this](int i) {
        if (i < 0) return;
        const QString id = tabs_->tabData(i).toString();
        if (id != current_) openRoom(id);
    });
    connect(tabs_, &QTabBar::tabCloseRequested, this, &MainWindow::closeTab);
    {
        auto *menu = new QMenu(plus_);
        menu->addAction("Start a direct message...", this, [this] {
            if (!startDm_) { startDm_ = new StartDmDialog(core_, this); startDm_->setAttribute(Qt::WA_DeleteOnClose); }
            startDm_->show(); startDm_->raise();
        });
        menu->addAction("Create a room...", this, [this] { auto *d = new CreateRoomDialog(core_, this); d->setAttribute(Qt::WA_DeleteOnClose); d->show(); });
        menu->addAction("Browse public rooms...", this, [this] {
            if (!browse_) { browse_ = new BrowseRoomsDialog(core_, this); browse_->setAttribute(Qt::WA_DeleteOnClose); }
            browse_->show(); browse_->raise();
        });
        menu->addSeparator();
        menu->addAction("Join a room by address...", this, [this] {
            bool ok = false;
            const QString id = QInputDialog::getText(this, "Join a room", "Room address or id (#room:server)", QLineEdit::Normal, QString(), &ok).trimmed();
            if (ok && !id.isEmpty()) core_->call("join_room", {{"address", id}});
        });
        plus_->setMenu(menu);
        plus_->setPopupMode(QToolButton::InstantPopup);
        plus_->setToolTip("Start a conversation, create or join a room");
    }
    connect(composer_, &QLineEdit::returnPressed, this, &MainWindow::send);
    connect(composer_, &QLineEdit::textEdited, this, [this](const QString &t) { if (!current_.isEmpty()) if (sendTyping_) core_->call("typing", {{"typing", !t.isEmpty()}}); });
    connect(attach_, &QToolButton::clicked, this, &MainWindow::chooseFiles);
    picker_ = new EmojiPicker(this);
    reactPicker_ = new EmojiPicker(this);
    reactPicker_->setCloseOnPick(true);
    connect(emojiBtn_, &QToolButton::clicked, this, [this] { picker_->popupAt(composer_->mapToGlobal(QPoint(composer_->width(), 0))); });
    connect(picker_, &EmojiPicker::emojiPicked, this, [this](const QString &g) { composer_->insert(g); composer_->setFocus(); emit composer_->textEdited(composer_->text()); });
    connect(reactPicker_, &EmojiPicker::emojiPicked, this, [this](const QString &g) { if (!reactEvent_.isEmpty()) core_->call("react", {{"event_id", reactEvent_}, {"key", g}}); });
    connect(composer_, &Composer::escapePressed, this, [this] { if (replyTo_.isEmpty() && editing_.isEmpty() && !pending_.isEmpty()) cancelAttachment(); else cancelContext(); });
    connect(composer_, &Composer::imagePasted, this, &MainWindow::stageImage);
    connect(composer_, &Composer::filesPasted, this, &MainWindow::stageFiles);

    connect(timeline_, &TimelineView::replyRequested, this, &MainWindow::startReply);
    connect(timeline_, &TimelineView::editRequested, this, &MainWindow::startEdit);
    connect(timeline_, &TimelineView::deleteRequested, this, [this](const QString &id) {
        if (QMessageBox::question(this, "Delete message", "Delete this message for everyone?") == QMessageBox::Yes) core_->call("redact", {{"event_id", id}});
    });
    connect(timeline_, &TimelineView::reactRequested, this, [this](const QString &id, const QString &key) { core_->call("react", {{"event_id", id}, {"key", key}}); });
    connect(timeline_, &TimelineView::reactPickerRequested, this, [this](const QString &id) { reactEvent_ = id; reactPicker_->popupAt(QCursor::pos() + QPoint(190, 0)); });
    connect(timeline_, &TimelineView::saveRequested, this, &MainWindow::saveAttachment);
        connect(timeline_, &TimelineView::loadReplyRequested, this, [this](const QString &id) { core_->call("load_reply", QJsonObject{{"event_id", id}}); });
        connect(timeline_, &TimelineView::olderRequested, this, [this] { core_->call("load_older"); });
    connect(timeline_, &TimelineView::pinRequested, this, [this](const QString &id, bool pin) { core_->call("pin_message", {{"event_id", id}, {"pinned", pin}}); });
    connect(timeline_, &TimelineView::bookmarkRequested, this, [this](const QString &id) { core_->call("toggle_bookmark", {{"event_id", id}}); });
    connect(timeline_, &TimelineView::pollVote, this, [this](const QString &id, const QString &answer) { core_->call("vote_poll", {{"poll_id", id}, {"answers", QJsonArray{answer}}}); });
    connect(timeline_, &TimelineView::pollEnd, this, [this](const QString &id) { core_->call("end_poll", {{"poll_id", id}}); });
    connect(timeline_, &TimelineView::textRequested, this, [this](const QString &id) { core_->call("fetch_text", {{"event_id", id}}); });
    connect(timeline_, &TimelineView::threadRequested, this, &MainWindow::openThread);
    connect(timeline_, &TimelineView::forwardRequested, this, [this](const QString &id) { auto *d = new ForwardDialog(core_, rooms_, id, this); d->setAttribute(Qt::WA_DeleteOnClose); d->show(); });
    connect(timeline_, &TimelineView::historyRequested, this, [this](const QString &id) { core_->call("edit_history", {{"event_id", id}}); });
    connect(timeline_, &TimelineView::playRequested, this, [this](const QString &id) { core_->call("fetch_media", {{"event_id", id}}); });
    connect(timeline_, &TimelineView::openRequested, this, &MainWindow::openPicture);
    composer_->candidates = [this](const QString &prefix) {
        QList<Composer::Candidate> out;
        for (const QJsonValue &v : details_["members"].toArray()) {
            const QJsonObject m = v.toObject();
            if (S(m, "user_id") == user_) continue;
            out.append({S(m, "user_id"), "@" + S(m, "name"), QString()});
        }
        const QString p = prefix.toLower();
        QList<Composer::Candidate> hits;
        for (const Composer::Candidate &c : out)
            if (p.isEmpty() || c.label.mid(1).toLower().startsWith(p) || c.id.mid(1).toLower().startsWith(p) || c.label.toLower().contains(p)) hits.append(c);
        if (hits.size() > 8) hits = hits.mid(0, 8);
        return hits;
    };
    loadDrafts();
}

MainWindow::~MainWindow() { saveDrafts(); }

QJsonObject MainWindow::roomRow(const QString &id) const
{
    for (const QJsonValue &v : rooms_) if (v.toObject().value("id").toString() == id) return v.toObject();
    return QJsonObject();
}

QString MainWindow::roomTitle(const QString &id) const { return S(roomRow(id), "title"); }

void MainWindow::buildMenus()
{
    QMenu *view = menuBar()->addMenu("&View"), *ws = menuBar()->addMenu("&Workspace"), *ch = menuBar()->addMenu("&Channel"),
          *tools = menuBar()->addMenu("&Tools"), *help = menuBar()->addMenu("&Help");
    QSettings st("vector", "vector");
    view->addAction("Toggle sidebar", QKeySequence("Ctrl+B"), this, [this] { sidebar_->setVisible(!sidebar_->isVisible()); });
    view->addAction("Search messages", QKeySequence("Ctrl+F"), this, [this] { toggleSearch(); });
    view->addAction("Member list", QKeySequence("Ctrl+M"), this, [this] { toggleMembers(); });
    view->addAction("Next tab", QKeySequence("Ctrl+Tab"), this, [this] { if (tabs_->count()) tabs_->setCurrentIndex((tabs_->currentIndex() + 1) % tabs_->count()); });
    view->addSeparator();
    actNotify_ = view->addAction("Desktop notifications");
    actNotify_->setCheckable(true);
    actNotify_->setChecked(notify_);
    connect(actNotify_, &QAction::toggled, this, [this](bool on) { notify_ = on; QSettings("vector", "vector").setValue("notifications", on); });
    actTyping_ = view->addAction("Send typing notifications");
    actTyping_->setCheckable(true);
    sendTyping_ = st.value("typing", true).toBool();
    actTyping_->setChecked(sendTyping_);
    connect(actTyping_, &QAction::toggled, this, [this](bool on) { sendTyping_ = on; if (!on) core_->call("typing", {{"typing", false}}); QSettings("vector", "vector").setValue("typing", on); });
    actPreviews_ = view->addAction("Link previews (asked from your homeserver)");
    actPreviews_->setCheckable(true);
    actPreviews_->setToolTip("The preview is fetched by your homeserver, which therefore sees the link");
    actPreviews_->setChecked(core_->boolPref("linkPreviews", false));
    connect(actPreviews_, &QAction::toggled, this, [this](bool on) { core_->setBoolPref("linkPreviews", on); core_->call("set_previews", {{"on", on}}); });
    actIndex_ = view->addAction("Search all my messages (keeps an encrypted index on this computer)");
    actIndex_->setCheckable(true);
    actIndex_->setChecked(core_->boolPref("messageIndex", false));
    connect(actIndex_, &QAction::toggled, this, [this](bool on) { core_->setBoolPref("messageIndex", on); core_->call("set_message_index", {{"on", on}}); searchPanel_->setIndexEnabled(on); });
    searchPanel_->setIndexEnabled(actIndex_->isChecked());
    actTray_ = view->addAction("Tray icon");
    actTray_->setCheckable(true);
    actTray_->setChecked(st.value("tray", true).toBool());
    actCloseTray_ = view->addAction("Keep running in the tray when the window is closed");
    actCloseTray_->setCheckable(true);
    actCloseTray_->setChecked(st.value("close_to_tray", false).toBool());
    connect(actTray_, &QAction::toggled, this, [this](bool on) {
        QSettings("vector", "vector").setValue("tray", on);
        if (tray_) tray_->setVisible(on);
        if (!on && actCloseTray_) actCloseTray_->setChecked(false);
    });
    connect(actCloseTray_, &QAction::toggled, this, [this](bool on) {
        QSettings("vector", "vector").setValue("close_to_tray", on);
        if (on && actTray_ && !actTray_->isChecked()) actTray_->setChecked(true);
    });
    ws->addAction("Start a direct message...", this, [this] { plus_->menu()->actions().at(0)->trigger(); });
    ws->addAction("Create a room...", this, [this] { plus_->menu()->actions().at(1)->trigger(); });
    ws->addAction("Browse public rooms...", this, [this] { plus_->menu()->actions().at(2)->trigger(); });
    ws->addSeparator();
    ws->addAction("Sign out", this, [this] { core_->call("sign_out"); });
    ws->addAction("Quit", QKeySequence::Quit, this, [] { qApp->quit(); });
    ch->addAction("Room settings...", this, [this] { roomSettings(); });
    ch->addAction("Invite someone...", this, [this] {
        if (current_.isEmpty()) return;
        bool ok = false;
        const QString id = QInputDialog::getText(this, "Invite", "Matrix id of the person (@name:server)", QLineEdit::Normal, QString(), &ok).trimmed();
        if (ok && !id.isEmpty()) core_->call("room_action", {{"kind", "invite"}, {"a", id}, {"b", ""}});
    });
    ch->addAction("Create poll...", this, [this] { if (!current_.isEmpty()) PollDialog(core_, this).exec(); });
    ch->addAction("Close tab", QKeySequence("Ctrl+W"), this, [this] { if (tabs_->currentIndex() >= 0) closeTab(tabs_->currentIndex()); });
    ch->addAction("Leave room", this, [this] { if (!current_.isEmpty() && QMessageBox::question(this, "Leave", "Leave " + roomTitle(current_) + "?") == QMessageBox::Yes) core_->call("leave_room", {{"room_id", current_}}); });
    tools->addAction("Preferences...", QKeySequence("Ctrl+,"), this, [this] { showPreferences(); });
    tools->addSeparator();
    tools->addAction("Saved messages...", QKeySequence("Ctrl+Shift+B"), this, [this] { showSaved(); });
    tools->addSeparator();
    tools->addAction("Verify this session...", this, [this] { core_->call("request_verification"); });
    tools->addAction("Enter recovery key...", this, [this] { showRecovery(false); });
    tools->addAction("Set up recovery...", this, [this] { showRecovery(true); });
    help->addAction("About qVector", this, [this] { showAbout(this); });
}

void MainWindow::showMain(bool main) { root_->setCurrentIndex(main ? 1 : 0); menuBar()->setVisible(main); statusBar()->setVisible(main); }

void MainWindow::start()
{
    showMain(false);
    core_->call("init");
}

void MainWindow::applyChrome()
{
    /* Nothing here hard-codes a colour or replaces the platform style wholesale: dividers are a blend of the theme's text and window colours
       (about #8c8c8c on a default light palette), and the tab bar is the theme's own style with flat tabs drawn over it. */
    const QPalette pal = QApplication::palette();
    const QColor line = QColor::fromRgbF(0.4 * pal.color(QPalette::WindowText).redF() + 0.6 * pal.color(QPalette::Window).redF(),
                                         0.4 * pal.color(QPalette::WindowText).greenF() + 0.6 * pal.color(QPalette::Window).greenF(),
                                         0.4 * pal.color(QPalette::WindowText).blueF() + 0.6 * pal.color(QPalette::Window).blueF());
    split_->setStyleSheet(QString("QSplitter::handle { background: %1; image: none; }").arg(line.name()));
    statusLine_->setStyleSheet(QString("background: %1;").arg(line.name()));
    for (QFrame *f : {statusSep1_, statusSep2_, statusSep3_}) f->setStyleSheet(QString("background: %1;").arg(line.name()));
    for (QFrame *f : {tabLine_, topicLine_, pinLine_}) f->setStyleSheet(QString("background: %1;").arg(line.name()));
    composer_->setStyleSheet(QString("QLineEdit { border: 1px solid %1; border-radius: 4px; padding: 6px 10px; background: palette(base); color: palette(text); font-size: 11pt; }"
                                     "QLineEdit:focus { border-color: palette(highlight); }").arg(line.name()));
    banner_->setStyleSheet(QString("background:%1;color:%2;padding:3px 8px").arg(pal.color(QPalette::ToolTipBase).name(), pal.color(QPalette::ToolTipText).name()));
    {
        auto draw = [&](bool smiley) {
            const int px = 48;
            QPixmap pm(px, px);
            pm.fill(Qt::transparent);
            QPainter p(&pm);
            p.setRenderHint(QPainter::Antialiasing);
            p.scale(px / 24.0, px / 24.0);
            p.setPen(Qt::NoPen);
            p.setBrush(pal.color(QPalette::WindowText));
            QPainterPath ring;
            ring.setFillRule(Qt::OddEvenFill);
            ring.addEllipse(QPointF(12, 12), 10, 10);
            ring.addEllipse(QPointF(12, 12), 8.5, 8.5);
            p.drawPath(ring);
            if (smiley) {
                p.drawEllipse(QPointF(8.5, 9.5), 1.5, 1.5);
                p.drawEllipse(QPointF(15.5, 9.5), 1.5, 1.5);
                QPainterPath m;
                m.moveTo(17, 14);
                m.lineTo(7, 14);
                m.cubicTo(7, 16.2, 9.25, 18, 12, 18);
                m.cubicTo(14.75, 18, 17, 16.2, 17, 14);
                p.drawPath(m);
            } else {
                p.drawRect(QRectF(11, 7, 2, 10));
                p.drawRect(QRectF(7, 11, 10, 2));
            }
            pm.setDevicePixelRatio(2.0);
            return QIcon(pm);
        };
        attach_->setIcon(draw(false));
        emojiBtn_->setIcon(draw(true));
    }
    const QString base = QApplication::style()->name();
    auto *cur = qobject_cast<QProxyStyle *>(tabs_->style());
    if (!cur || !cur->baseStyle() || cur->baseStyle()->name() != base) {
        auto *st = new FlatTabStyle(base);
        st->setLine(line);
        st->setParent(tabs_);
        tabs_->setStyle(st);
    } else if (auto *flat = dynamic_cast<FlatTabStyle *>(cur)) { flat->setLine(line); tabs_->update(); }
}

bool MainWindow::event(QEvent *e)
{
    if (e->type() == QEvent::ApplicationPaletteChange || e->type() == QEvent::ThemeChange) applyChrome();
    if (e->type() == QEvent::WindowActivate) core_->call("set_focus", {{"focused", true}});
    else if (e->type() == QEvent::WindowDeactivate) core_->call("set_focus", {{"focused", false}});
    return QMainWindow::event(e);
}

void MainWindow::closeEvent(QCloseEvent *e)
{
    if (tray_ && tray_->isVisible() && actCloseTray_ && actCloseTray_->isChecked()) { hide(); e->ignore(); return; }
    saveDrafts();
    QMainWindow::closeEvent(e);
}

void MainWindow::onEvent(const QString &name, const QJsonValue &p)
{
    if (name == "state") {
        const QJsonObject o = p.toObject();
        const QString screen = S(o, "screen");
        login_->refresh(screen, S(o, "status"), o["busy"].toBool());
        showMain(screen == "main");
        if (screen == "main") { core_->call("set_previews", {{"on", core_->boolPref("linkPreviews", false)}}); user_ = core_->call("account_info").toObject()["user_id"].toString(); workspace_ = core_->call("account_info").toObject()["homeserver"].toString(); updateStatus(); }
        else { current_.clear(); rooms_ = QJsonArray(); while (tabs_->count()) tabs_->removeTab(0); }
    } else if (name == "rooms") {
        rooms_ = p.toArray();
        sidebar_->refresh(rooms_, current_, user_ + " - " + workspace_);
        updateTabs();
        updateTitle();
        const QJsonObject cur = roomRow(current_);
        inviteBar_->setVisible(!cur.isEmpty() && cur["invite"].toBool());
    } else if (name == "timeline") {
        const QJsonObject o = p.toObject();
        if (S(o, "room_id") != current_) return;
        timeline_->setMe(user_);
        timeline_->setRows(o["rows"].toArray());
        pinned_.clear();
        for (const QJsonValue &v : o["rows"].toArray()) if (v.toObject()["pinned"].toBool()) pinned_.append(v.toObject());
        updatePinBar();
    } else if (name == "details") {
        details_ = p.toObject();
        if (S(details_, "id") == current_) { timeline_->setEncrypted(details_["encrypted"].toBool()); updateTopic(); if (memberList_->isVisible()) memberList_->refresh(details_); }
    } else if (name == "typing") {
        QStringList names;
        for (const QJsonValue &v : p.toArray()) names << v.toString();
        typingLabel_->setText(names.isEmpty() ? QString() : names.size() == 1 ? names[0] + " is typing..." : names.size() == 2 ? names[0] + " and " + names[1] + " are typing..." : "Several people are typing...");
    } else if (name == "older") {
        timeline_->setHistoryState(!p.toObject()["reached"].toBool(), false);
        timeline_->refresh();
    } else if (name == "notice") {
        statusBar()->showMessage(p.toString(), 6000);
    } else if (name == "session") {
        session_ = p.toObject();
        updateStatus();
    } else if (name == "users") {
        if (startDm_) startDm_->setUsers(p.toArray());
    } else if (name == "directory") {
        if (browse_) browse_->setRooms(p.toArray());
    } else if (name == "edit_history") {
        showEditHistory(this, p.toArray());
    } else if (name == "verification") {
        showVerify(p.toObject());
    } else if (name == "recovery") {
        if (recoveryDlg_) recoveryDlg_->setState(p.toObject());
    } else if (name == "sessions") {
        sessions_ = p.toArray(); /* the open Preferences dialog reads it on its timer */
    } else if (name == "bookmarks") {
        bookmarks_ = p.toArray();
        bookmarkIds_.clear();
        for (const QJsonValue &v : bookmarks_) bookmarkIds_.insert(S(v.toObject(), "event_id"));
        timeline_->setBookmarks(bookmarkIds_);
        threadView_->setBookmarks(bookmarkIds_);
        timeline_->refresh();
        if (savedDlg_) savedDlg_->setBookmarks(bookmarks_);
    } else if (name == "search") {
        searchPanel_->setResults(p.toArray());
    } else if (name == "thread") {
        threadView_->setMe(user_);
        threadView_->setRows(p.toArray());
    } else if (name == "text_file") {
        const QJsonObject t = p.toObject();
        auto *v = new TextViewer(S(t, "name"), S(t, "text"), qint64(t["size"].toDouble()), t["truncated"].toBool(), this);
        v->setAttribute(Qt::WA_DeleteOnClose);
        const QString id = S(t, "event_id");
        connect(v, &TextViewer::saveRequested, this, [this, id] { saveAttachment(id); });
        v->show();
    } else if (name == "media_file") {
        const QString id = S(p.toObject(), "event_id");
        const QJsonObject row = timeline_->row(id);
        if (S(row, "kind") == "audio") (new AudioPlayer(S(p.toObject(), "path"), S(row, "file_name").isEmpty() ? S(row, "body") : S(row, "file_name"), this))->show(); /* a window with the waveform */
        else timeline_->playFile(id, S(p.toObject(), "path"));
    } else if (name == "open_file") {
        QDesktopServices::openUrl(QUrl::fromLocalFile(p.toString()));
    } else if (name == "alerts") {
        if (!notify_) return;
        for (const QJsonValue &v : p.toArray()) {
            const QJsonObject a = v.toObject();
            notifier_->show(S(a, "room_id"), S(a, "title"), QString("%1 new message(s)").arg(a["new"].toInt()), a["highlight"].toBool());
        }
    }
}

void MainWindow::openRoom(const QString &id)
{
    if (id.isEmpty()) return;
    cancelAttachment();
    if (!current_.isEmpty()) drafts_[current_] = composer_->text();
    cancelContext();
    if (id != current_) closeThread();
    current_ = id;
    searchPanel_->setCurrentRoom(id);
    int idx = tabIndex(id);
    if (idx < 0) { idx = tabs_->addTab(roomTitle(id)); tabs_->setTabData(idx, id); }
    { QSignalBlocker b(tabs_); tabs_->setCurrentIndex(idx); }
    timeline_->reset();
    timeline_->setHistoryState(true, false);
    timelines_->setCurrentWidget(timeline_);
    details_ = QJsonObject();
    typingLabel_->clear();
    composer_->setText(drafts_.value(id));
    core_->call("select_room", {{"room_id", id}});
    const QJsonObject row = roomRow(id);
    inviteBar_->setVisible(row["invite"].toBool());
    composer_->setEnabled(!row["invite"].toBool());
    sidebar_->setCurrent(id);
    updateTopic();
    updateTitle();
    updateTabs();
    composer_->setFocus();
    if (!pendingReveal_.isEmpty()) QTimer::singleShot(900, this, [this] { const QString id = pendingReveal_; pendingReveal_.clear(); timeline_->revealMessage(id); });
}

void MainWindow::openRoomByTitle(const QString &title)
{
    for (const QJsonValue &v : rooms_) if (v.toObject()["title"].toString() == title) { openRoom(v.toObject()["id"].toString()); return; }
}

int MainWindow::tabIndex(const QString &id) const
{
    for (int i = 0; i < tabs_->count(); i++) if (tabs_->tabData(i).toString() == id) return i;
    return -1;
}

void MainWindow::closeTab(int index)
{
    const QString id = tabs_->tabData(index).toString();
    QSignalBlocker b(tabs_);
    tabs_->removeTab(index);
    if (id == current_) {
        current_.clear();
        timelines_->setCurrentWidget(empty_);
        if (tabs_->count()) openRoom(tabs_->tabData(qMin(index, tabs_->count() - 1)).toString());
        else { updateTopic(); updateTitle(); }
    }
}

void MainWindow::updateTabs()
{
    const QPalette pal = palette();
    for (int i = 0; i < tabs_->count(); i++) {
        const QString id = tabs_->tabData(i).toString(), title = roomTitle(id);
        if (title.isEmpty()) continue;
        if (tabs_->tabText(i) != title) tabs_->setTabText(i, title);
        tabs_->setTabIcon(i, QIcon(profilePixmap(roomRow(id)["avatar_path"].toString(), id, title, 16, devicePixelRatioF(), pal)));
    }
}

void MainWindow::updateTitle()
{
    int unread = 0;
    for (const QJsonValue &v : rooms_) { const QJsonObject r = v.toObject(); if (!r["invite"].toBool()) unread += r["unread"].toInt(); }
    const QString t = current_.isEmpty() ? QString("qVector") : roomTitle(current_) + " - qVector";
    setWindowTitle(unread > 0 ? QString("(%1) ").arg(unread) + t : t);
    if (tray_) tray_->setIcon(QIcon(trayPixmap(unread, 64)));
}

void MainWindow::updateTopic()
{
    topic_->setText(S(details_, "topic").simplified());
    const int n = details_["members"].toArray().size();
    membersBtn_->setText(current_.isEmpty() ? QString() : QString("%1 members").arg(n));
    membersBtn_->setVisible(!current_.isEmpty());
}

void MainWindow::updateClock()
{
    statusClock_->setText(formatWhen(QDateTime::currentDateTime(), true, false));
}

void MainWindow::updateStatus()
{
    statusLeft_->setText(user_);
    const bool verified = session_["verified"].toBool();
    statusRight_->setText(session_.isEmpty() ? QString() : verified ? "Session verified" : "Session not verified");
    statusSep1_->setVisible(!session_.isEmpty());
    statusSep2_->setVisible(!session_.isEmpty());
    QString banner;
    if (session_["has_identity"].toBool() && !verified) banner = "This session is not verified: other people cannot trust it and old messages may stay unreadable.";
    else if (S(session_, "recovery") == "incomplete") banner = "Enter your recovery key to read old messages.";
    else if (S(session_, "recovery") == "disabled") banner = "This account has no recovery key: if you lose all your sessions, your encrypted messages are gone.";
    banner_->setText(banner);
    banner_->setVisible(!banner.isEmpty());
}

void MainWindow::showContextBar(const QString &text)
{
    contextLabel_->setText(text);
    contextBar_->setVisible(!text.isEmpty());
}

void MainWindow::cancelContext()
{
    if (!editing_.isEmpty()) composer_->clear();
    replyTo_.clear();
    editing_.clear();
    showContextBar(QString());
}

void MainWindow::startReply(const QString &eventId)
{
    cancelContext();
    const QJsonObject r = timeline_->row(eventId);
    QString snip = S(r, "body").simplified();
    if (snip.size() > 100) snip = snip.left(100) + "...";
    replyTo_ = eventId;
    showContextBar("Replying to " + S(r, "sender") + (snip.isEmpty() ? QString() : ": " + snip));
    composer_->setFocus();
}

void MainWindow::startEdit(const QString &eventId)
{
    cancelContext();
    const QJsonObject r = timeline_->row(eventId);
    if (r.isEmpty()) return;
    editing_ = eventId;
    showContextBar("Editing your message - Enter to save, Esc to cancel");
    composer_->setText(S(r, "body"));
    composer_->setFocus();
}

void MainWindow::send()
{
    const QString text = composer_->text();
    if (current_.isEmpty()) return;
    if (!pending_.isEmpty()) {
        /* several files go out as one gallery message; the typed text is its caption */
        core_->call("send_files", {{"paths", QJsonArray::fromStringList(pending_)}, {"caption", text}});
        composer_->clear(); drafts_.remove(current_);
        cancelAttachment();
        timeline_->stickToBottom();
        return;
    }
    if (text.isEmpty()) return;
    if (!editing_.isEmpty()) {
        core_->call("edit", {{"event_id", editing_}, {"text", text}});
        editing_.clear(); composer_->clear(); showContextBar(QString());
        return;
    }
    core_->call("send", {{"text", text}, {"reply_to", replyTo_}});
    replyTo_.clear(); composer_->clear(); showContextBar(QString()); drafts_.remove(current_);
    timeline_->stickToBottom();
}

void MainWindow::chooseFiles()
{
    if (current_.isEmpty()) return;
    stageFiles(QFileDialog::getOpenFileNames(this, "Send files"));
}

/* Nothing is sent until Enter: the files wait in a bar above the message line, where a caption can be typed. */
void MainWindow::stageFiles(const QStringList &paths)
{
    if (current_.isEmpty() || paths.isEmpty()) return;
    cancelContext();
    for (const QString &p : paths) if (!pending_.contains(p)) pending_ << p; /* pasting or choosing more adds to the same message */
    QString what;
    if (pending_.size() == 1) {
        const QFileInfo fi(pending_[0]);
        what = fi.fileName() + " (" + QLocale().formattedDataSize(fi.size()) + ")";
        QImage img(pending_[0]);
        if (!img.isNull()) attachThumb_->setPixmap(QPixmap::fromImage(img.scaled(48, 48, Qt::KeepAspectRatio, Qt::SmoothTransformation)));
        else attachThumb_->clear();
    } else {
        what = QString::number(pending_.size()) + " files as one message";
        attachThumb_->clear();
    }
    attachLabel_->setText("Ready to send " + what + " - type a caption if you like, Enter to send, Esc to cancel");
    attachBar_->show();
    composer_->setFocus();
}

void MainWindow::stageImage(const QImage &img)
{
    if (!pasteDir_) pasteDir_ = new QTemporaryDir;
    if (!pasteDir_->isValid()) { statusLeft_->setText("Cannot keep the picture: no temporary folder"); return; }
    const QString path = pasteDir_->filePath("image-" + QDateTime::currentDateTime().toString("yyyyMMdd-hhmmss-zzz") + ".png");
    if (!img.save(path, "PNG")) { statusLeft_->setText("Cannot keep the picture"); return; }
    stageFiles({path});
}

void MainWindow::cancelAttachment()
{
    pending_.clear();
    attachBar_->hide();
}

void MainWindow::dragEnterEvent(QDragEnterEvent *e)
{
    if (e->mimeData()->hasUrls() || e->mimeData()->hasImage()) e->acceptProposedAction();
}

void MainWindow::dropEvent(QDropEvent *e)
{
    QStringList files;
    for (const QUrl &u : e->mimeData()->urls()) if (u.isLocalFile() && QFileInfo(u.toLocalFile()).isFile()) files << u.toLocalFile();
    if (!files.isEmpty()) stageFiles(files);
    else if (e->mimeData()->hasImage()) stageImage(qvariant_cast<QImage>(e->mimeData()->imageData()));
}

void MainWindow::saveAttachment(const QString &eventId)
{
    const QJsonObject r = timeline_->row(eventId);
    const QString dest = QFileDialog::getSaveFileName(this, "Save as", QDir::homePath() + "/" + S(r, "file_name"));
    if (!dest.isEmpty()) core_->call("save_attachment", {{"event_id", eventId}, {"dest", dest}});
}


void MainWindow::toggleMembers()
{
    memberList_->setVisible(!memberList_->isVisible());
    if (memberList_->isVisible()) { memberList_->setMe(user_); memberList_->refresh(details_); }
    layoutPanes();
}

void MainWindow::toggleSearch()
{
    searchPanel_->setVisible(!searchPanel_->isVisible());
    layoutPanes();
    if (searchPanel_->isVisible()) searchPanel_->focusInput();
}

void MainWindow::layoutPanes()
{
    QList<int> sz = split_->sizes();
    const bool m = memberList_->isVisible(), t = threadPanel_->isVisible(), f = searchPanel_->isVisible();
    int total = 0;
    for (int v : sz) total += v;
    const int side = sidebar_->isVisible() ? qMax(180, sz.value(0)) : 0;
    const int mem = m ? 190 : 0, thr = t ? 320 : 0, srch = f ? 320 : 0;
    split_->setSizes({side, qMax(240, total - side - mem - thr - srch), mem, thr, srch});
}

void MainWindow::openThread(const QString &rootId)
{
    const QJsonObject root = timeline_->row(rootId);
    threadTitle_->setText("Thread - " + S(root, "sender"));
    threadView_->reset();
    threadView_->setMe(user_);
    threadPanel_->show();
    layoutPanes();
    core_->call("open_thread", {{"root_id", rootId}});
    threadInput_->setFocus();
}

void MainWindow::closeThread()
{
    if (!threadPanel_->isVisible()) return;
    threadPanel_->hide();
    layoutPanes();
    core_->call("close_thread");
}

void MainWindow::sendThreadReply()
{
    const QString t = threadInput_->text().trimmed();
    if (t.isEmpty()) return;
    core_->call("send_thread", {{"text", t}});
    threadInput_->clear();
}

void MainWindow::updatePinBar()
{
    pinIndex_ = qBound(0, pinIndex_, qMax(0, int(pinned_.size()) - 1));
    pinBar_->setVisible(!pinned_.isEmpty());
    pinLine_->setVisible(!pinned_.isEmpty());
    if (pinned_.isEmpty()) return;
    const QJsonObject r = pinned_[pinIndex_];
    QString t = S(r, "body").simplified();
    if (t.size() > 140) t = t.left(140) + "...";
    pinText_->setText(QString("Pinned %1/%2   %3: %4").arg(pinIndex_ + 1).arg(pinned_.size()).arg(S(r, "sender"), t));
    pinPrev_->setEnabled(pinIndex_ > 0);
    pinNext_->setEnabled(pinIndex_ + 1 < pinned_.size());
}

void MainWindow::showSaved()
{
    if (!savedDlg_) {
        savedDlg_ = new SavedDialog(core_, this);
        savedDlg_->setAttribute(Qt::WA_DeleteOnClose);
        connect(savedDlg_, &SavedDialog::showRequested, this, &MainWindow::jumpTo);
    }
    savedDlg_->setBookmarks(bookmarks_);
    savedDlg_->show();
    savedDlg_->raise();
}

void MainWindow::jumpTo(const QString &room, const QString &event)
{
    if (room != current_) { pendingReveal_ = event; openRoom(room); return; }
    timeline_->revealMessage(event);
}

void MainWindow::verifyPerson(const QString &userId) { core_->call("request_user_verification", {{"user_id", userId}}); }

void MainWindow::showVerify(const QJsonObject &state)
{
    if (!verifyDlg_) { verifyDlg_ = new VerifyDialog(core_, this); verifyDlg_->setAttribute(Qt::WA_DeleteOnClose); }
    verifyDlg_->setState(state);
    if (autoConfirm_ && S(state, "state") == "emoji") core_->call("confirm_verification");
}

void MainWindow::showRecovery(bool create)
{
    if (recoveryDlg_) recoveryDlg_->deleteLater();
    recoveryDlg_ = new RecoveryDialog(core_, create, this);
    recoveryDlg_->setAttribute(Qt::WA_DeleteOnClose);
    recoveryDlg_->show();
}

void MainWindow::roomSettings()
{
    if (current_.isEmpty()) return;
    if (!details_["can_edit"].toBool()) { QMessageBox::information(this, "Room settings", "You are not allowed to change this room's name or topic."); return; }
    RoomSettingsDialog(core_, details_, this).exec();
}

void MainWindow::openPicture(const QString &eventId)
{
    const QJsonObject r = timeline_->row(eventId);
    const QString kind = S(r, "kind");
    if (kind == "image" && !S(r, "image_path").isEmpty()) {
        auto *v = new ImageViewer(S(r, "image_path"), S(r, "file_name"), this);
        v->setAttribute(Qt::WA_DeleteOnClose);
        connect(v, &ImageViewer::saveRequested, this, [this, eventId] { saveAttachment(eventId); });
        v->show();
    } else if (kind == "audio") core_->call("fetch_media", {{"event_id", eventId}});
    else core_->call("open_attachment", {{"event_id", eventId}});
}

void MainWindow::showPreferences()
{
    auto *dlg = new QDialog(this);
    dlg->setAttribute(Qt::WA_DeleteOnClose);
    dlg->setWindowTitle("Preferences");
    prefsDlg_ = dlg;
    auto *v = new QVBoxLayout(dlg);
    auto group = [&](const QString &title, std::initializer_list<QAction *> acts) {
        auto *box = new QGroupBox(title);
        auto *bl = new QVBoxLayout(box);
        for (QAction *a : acts) {
            auto *cb = new QCheckBox(a->text());
            cb->setChecked(a->isChecked());
            cb->setToolTip(a->toolTip());
            connect(cb, &QCheckBox::toggled, a, &QAction::setChecked);
            connect(a, &QAction::toggled, cb, [cb](bool on) { QSignalBlocker b(cb); cb->setChecked(on); });
            bl->addWidget(cb);
        }
        v->addWidget(box);
    };
    group("Notifications", {actNotify_});
    group("Privacy", {actTyping_, actPreviews_});
    group("Search", {actIndex_});
    group("Window", {actTray_, actCloseTray_});
    {   /* Timestamps: presets to pick from, or any Qt format string typed in; the example below shows the result at once */
        auto *box = new QGroupBox("Timestamps");
        auto *form = new QFormLayout(box);
        auto *date = new QComboBox, *time = new QComboBox;
        date->setEditable(true); time->setEditable(true);
        date->addItems({"dd/MM/yyyy", "yyyy-MM-dd", "MM/dd/yyyy", "d MMM yyyy", "ddd d MMM yyyy", "dd.MM.yyyy"});
        time->addItems({"HH:mm", "h:mm AP", "HH:mm:ss"});
        date->setCurrentText(dateFormat());
        time->setCurrentText(timeFormat());
        auto *example = new QLabel;
        form->addRow("Date format", date);
        form->addRow("Time format", time);
        form->addRow("Example", example);
        auto apply = [this, date, time, example] {
            setTimestampFormats(date->currentText(), time->currentText());
            example->setText(formatWhen(QDateTime::currentDateTime().addDays(-3), true));
            updateClock();
            timeline_->refresh();
            threadView_->refresh();
        };
        connect(date, &QComboBox::currentTextChanged, dlg, apply);
        connect(time, &QComboBox::currentTextChanged, dlg, apply);
        example->setText(formatWhen(QDateTime::currentDateTime().addDays(-3), true));
        v->addWidget(box);
    }
    {   /* the account's sessions, as the server knows them */
        auto *box = new QGroupBox("Your sessions");
        auto *bl = new QVBoxLayout(box);
        auto *list = new QLabel("Loading...");
        list->setTextFormat(Qt::PlainText);
        bl->addWidget(list);
        auto fill = [this, list] {
            QStringList lines;
            for (const QJsonValue &sv : sessions_) {
                const QJsonObject s = sv.toObject();
                lines << QString("%1  (%2)%3  -  %4").arg(S(s, "name"), S(s, "id"), s["current"].toBool() ? "  - this session" : "", s["verified"].toBool() ? "verified" : "not verified");
            }
            list->setText(lines.isEmpty() ? "Loading..." : lines.join("\n"));
        };
        fill();
        auto *timer = new QTimer(dlg);
        connect(timer, &QTimer::timeout, dlg, fill);
        timer->start(700);
        v->addWidget(box);
        core_->call("load_sessions");
    }
    auto *close = new QPushButton("Close");
    close->setDefault(true);
    connect(close, &QPushButton::clicked, dlg, &QDialog::accept);
    v->addWidget(close, 0, Qt::AlignRight);
    dlg->show();
}

void MainWindow::saveDrafts()
{
    if (!current_.isEmpty()) drafts_[current_] = composer_->text();
    QSettings st("vector", "vector");
    st.beginGroup("drafts");
    st.remove("");
    for (auto it = drafts_.cbegin(); it != drafts_.cend(); ++it) if (!it.value().isEmpty()) st.setValue(QString::fromLatin1(it.key().toUtf8().toBase64()), it.value());
    st.endGroup();
}

void MainWindow::loadDrafts()
{
    QSettings st("vector", "vector");
    st.beginGroup("drafts");
    for (const QString &k : st.childKeys()) drafts_.insert(QString::fromUtf8(QByteArray::fromBase64(k.toLatin1())), st.value(k).toString());
    st.endGroup();
}

void MainWindow::threadForDemo(const QString &text)
{
    for (const QJsonValue &v : timeline_->rows())
        if (S(v.toObject(), "body").contains(text)) { openThread(S(v.toObject(), "id")); QTimer::singleShot(600, this, [this] { core_->call("send_thread", {{"text", "A reply **inside** the thread"}}); }); return; }
    QTimer::singleShot(1000, this, [this, text] { threadForDemo(text); }); /* the messages may not have arrived yet */
}

void MainWindow::dialogForDemo(const QString &which)
{
    if (which == "poll") PollDialog(core_, this).open();
    else if (which == "saved") showSaved();
    else if (which == "prefs") showPreferences();
    else if (which == "recovery") showRecovery(false);
    else if (which == "verify") showVerify(QJsonObject{{"state", "emoji"}, {"user", "@zach:example.org"}, {"emoji", QJsonArray{QJsonArray{"\U0001F436", "Dog"}, QJsonArray{"\U0001F431", "Cat"}, QJsonArray{"\U0001F981", "Lion"}, QJsonArray{"\U0001F40E", "Horse"}, QJsonArray{"\U0001F984", "Unicorn"}, QJsonArray{"\U0001F437", "Pig"}, QJsonArray{"\U0001F418", "Elephant"}}}, {"decimals", QJsonArray{123, 456, 789}}});
    else if (which == "settings") roomSettings();
    else if (which == "paste") { /* dev aid: act as if a picture was pasted, to see the caption bar */
        QImage img(320, 200, QImage::Format_RGB32);
        img.fill(QColor("#3b7dd8"));
        stageImage(img);
        composer_->setText("a caption");
    }
    else if (which.startsWith("audio:")) (new AudioPlayer(which.mid(6), QFileInfo(which.mid(6)).fileName(), this))->show(); /* dev aid: --dialog audio:FILE */
    else if (which == "text") {
        for (const QJsonValue &v : timeline_->rows()) if (v.toObject()["text_file"].toBool()) { core_->call("fetch_text", {{"event_id", S(v.toObject(), "id")}}); return; }
        QTimer::singleShot(1000, this, [this] { dialogForDemo("text"); }); /* the file may not have arrived yet */
    }
}

}
