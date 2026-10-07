#include "qt/webembed.h"
#include <QDesktopServices>
#include <cstdio>
#include <QToolButton>
#include <QUrl>
#include <QWebEngineFullScreenRequest>
#include <QWebEnginePage>
#include <QWebEngineProfile>
#include <QWebEngineSettings>
#include <QWebEngineView>

namespace vc {

namespace {

/* the page of the embedded player: it stays on YouTube's embed address; every link out of it opens in the system browser */
class EmbedPage : public QWebEnginePage {
public:
    using QWebEnginePage::QWebEnginePage;

protected:
    bool acceptNavigationRequest(const QUrl &url, NavigationType type, bool isMainFrame) override
    {
        if (!isMainFrame) return true; /* the player's own frames */
        if (type == NavigationTypeTyped || url.scheme() == "data" || url.scheme() == "about") return true; /* our own page */
        if (type == NavigationTypeLinkClicked && (url.scheme() == "https" || url.scheme() == "http")) QDesktopServices::openUrl(url);
        return false;
    }
    QWebEnginePage *createWindow(WebWindowType) override { return nullptr; } /* "watch on YouTube" and other pop-ups: not here */
    /* YouTube's own page complains about browser features this engine does not know ("Permissions-Policy header: Unrecognized feature", WebGPU):
       nothing to act on, so the messages are not printed unless the log is switched on */
    void javaScriptConsoleMessage(JavaScriptConsoleMessageLevel, const QString &message, int, const QString &) override
    {
        if (qEnvironmentVariableIsSet("VC_TIMELINE_LOG")) fprintf(stderr, "web: %s\n", message.toUtf8().constData());
    }
};

/* no cookies, no history, nothing kept on disk: a profile without a storage name is off the record */
QWebEngineProfile *embedProfile()
{
    static QWebEngineProfile *profile = new QWebEngineProfile();
    return profile;
}

}

WebEmbed::WebEmbed(const QString &videoId, QWidget *parent) : QWidget(parent)
{
    setAttribute(Qt::WA_DeleteOnClose, false);
    view_ = new QWebEngineView(this);
    auto *page = new EmbedPage(embedProfile(), view_);
    page->settings()->setAttribute(QWebEngineSettings::PlaybackRequiresUserGesture, false); /* the click on the card was the gesture */
    page->settings()->setAttribute(QWebEngineSettings::FullScreenSupportEnabled, false);
    page->settings()->setAttribute(QWebEngineSettings::JavascriptCanOpenWindows, false);
    connect(page, &QWebEnginePage::fullScreenRequested, page, [](QWebEngineFullScreenRequest r) { r.reject(); });
    view_->setPage(page);
    view_->setContextMenuPolicy(Qt::NoContextMenu);
    /* YouTube refuses an embed that does not say who embeds it (error 153): the page that holds the frame has an https address, which becomes the referrer */
    const QString html = QStringLiteral("<html><body style=\"margin:0;background:#000\"><iframe src=\"https://www.youtube-nocookie.com/embed/%1?autoplay=1&rel=0&playsinline=1\" "
                                        "style=\"border:0;position:absolute;left:0;top:0;width:100%;height:100%\" allow=\"autoplay; encrypted-media; picture-in-picture\" "
                                        "referrerpolicy=\"strict-origin-when-cross-origin\"></iframe></body></html>").arg(videoId);
    page->setHtml(html, QUrl(QStringLiteral("https://vector.invalid/")));
    close_ = new QToolButton(this);
    close_->setText(QStringLiteral("\u2715"));
    close_->setToolTip("Stop");
    close_->setAutoRaise(false);
    close_->setStyleSheet("QToolButton { background: rgba(0,0,0,150); color: white; border: none; padding: 3px 7px; border-radius: 3px; }"
                          "QToolButton:hover { background: rgba(200,0,0,200); }");
    connect(close_, &QToolButton::clicked, this, &WebEmbed::closeRequested);
}

void WebEmbed::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    view_->setGeometry(rect());
    close_->adjustSize();
    close_->move(width() - close_->width() - 6, 6);
    close_->raise();
}

}
