#ifndef VC_QT_LOGINPAGE_H
#define VC_QT_LOGINPAGE_H

#include "qt/core.h"
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QStackedWidget>
#include <QCheckBox>

namespace vc {

/* The sign-in card and the unlock card (saved session sealed with a passphrase). */
class LoginPage : public QWidget {
    Q_OBJECT
public:
    LoginPage(Core *core, QWidget *parent = nullptr);
    /* screen: "login" or "unlock"; status: what to tell the user (empty: nothing) */
    void refresh(const QString &screen, const QString &status, bool busy);

private:
    Core *core_;
    QStackedWidget *stack_;
    QLineEdit *srv_, *usr_, *pwd_, *pp_, *unlockPp_;
    QPushButton *loginBtn_, *unlockBtn_;
    QLabel *loginStatus_, *unlockStatus_;
    void doLogin();
};

}

#endif
