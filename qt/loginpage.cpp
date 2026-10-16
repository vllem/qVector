#include "qt/theme.h"
#include "qt/uiscale.h"
#include "qt/loginpage.h"
#include <QFormLayout>
#include <QFrame>
#include <QVBoxLayout>

namespace vc {

static QFrame *card(const QString &title, QVBoxLayout **inner)
{
    auto *f = new QFrame;
    f->setFrameShape(QFrame::StyledPanel);
    auto *v = new QVBoxLayout(f);
    auto *t = new QLabel(title);
    onUiScale(f, [f, v, t] {
        f->setFixedWidth(px(400));
        v->setContentsMargins(px(24), px(22), px(24), px(22));
        v->setSpacing(px(10));
        QFont ft = scaledFont(QApplication::font(), 1.5); /* a size of its own: made again when the zoom changes */
        ft.setBold(true);
        t->setFont(ft);
    });
    v->addWidget(t);
    *inner = v;
    return f;
}

LoginPage::LoginPage(Core *core, QWidget *parent) : QWidget(parent), core_(core)
{
    auto *outer = new QVBoxLayout(this);
    stack_ = new QStackedWidget;
    outer->addStretch(1);
    outer->addWidget(stack_, 0, Qt::AlignHCenter);
    outer->addStretch(1);

    QVBoxLayout *in;
    QFrame *signin = card("Sign in to Matrix", &in);
    auto *form = new QFormLayout;
    srv_ = new QLineEdit("matrix.org");
    usr_ = new QLineEdit;
    pwd_ = new QLineEdit;
    pwd_->setEchoMode(QLineEdit::Password);
    pp_ = new QLineEdit;
    pp_->setEchoMode(QLineEdit::Password);
    pp_->setPlaceholderText("optional - leave empty to stay signed in");
    form->addRow("Server", srv_);
    form->addRow("User", usr_);
    form->addRow("Password", pwd_);
    form->addRow("Passphrase", pp_);
    in->addLayout(form);
    loginStatus_ = new QLabel;
    loginStatus_->setWordWrap(true);
    in->addWidget(loginStatus_);
    loginBtn_ = new QPushButton("Sign in");
    loginBtn_->setDefault(true);
    in->addWidget(loginBtn_);
    stack_->addWidget(signin);
    for (QLineEdit *e : {srv_, usr_, pwd_, pp_}) connect(e, &QLineEdit::returnPressed, this, &LoginPage::doLogin);
    connect(loginBtn_, &QPushButton::clicked, this, &LoginPage::doLogin);

    QFrame *unlock = card("Unlock", &in);
    in->addWidget(new QLabel("Enter your passphrase to open the saved session"));
    unlockPp_ = new QLineEdit;
    unlockPp_->setEchoMode(QLineEdit::Password);
    in->addWidget(unlockPp_);
    unlockStatus_ = new QLabel;
    unlockStatus_->setWordWrap(true);
    in->addWidget(unlockStatus_);
    unlockBtn_ = new QPushButton("Unlock");
    auto *other = new QPushButton("Use a different account");
    in->addWidget(unlockBtn_);
    in->addWidget(other);
    stack_->addWidget(unlock);
    auto doUnlock = [this] { core_->call("unlock", {{"passphrase", unlockPp_->text()}}); };
    connect(unlockBtn_, &QPushButton::clicked, this, doUnlock);
    connect(unlockPp_, &QLineEdit::returnPressed, this, doUnlock);
    connect(other, &QPushButton::clicked, this, [this] { core_->call("sign_out"); });
}

void LoginPage::doLogin()
{
    core_->call("login", {{"homeserver", srv_->text().trimmed()}, {"user", usr_->text().trimmed()}, {"password", pwd_->text()}, {"passphrase", pp_->text()}});
}

void LoginPage::refresh(const QString &screen, const QString &status, bool busy)
{
    const bool unlock = screen == "unlock";
    stack_->setCurrentIndex(unlock ? 1 : 0);
    QLabel *l = unlock ? unlockStatus_ : loginStatus_;
    (unlock ? loginStatus_ : unlockStatus_)->clear();
    l->setText(status);
    const bool error = !status.isEmpty() && !busy;
    l->setStyleSheet(error ? "color:" + errorColor(qApp->palette()).name() : QString());
    loginBtn_->setEnabled(!busy);
    loginBtn_->setText(busy ? "Signing in..." : "Sign in");
    unlockBtn_->setEnabled(!busy);
    if (unlock && error) { unlockPp_->clear(); unlockPp_->setFocus(); }
    if (!busy && !unlock && !error) pwd_->clear();
}

}
