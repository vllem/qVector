#ifndef VC_QT_WEBEMBED_H
#define VC_QT_WEBEMBED_H

#include <QString>
#include <QWidget>

class QToolButton;
class QWebEngineView;

namespace vc {

/* A YouTube video played by YouTube's own embedded player (the page youtube-nocookie.com/embed/<id>) in a small browser view (Qt WebEngine),
   the way chat applications built on a browser do it. Only built when Qt WebEngine is available (VC_HAVE_WEBENGINE). The view keeps no cookies or
   history (an off-the-record profile) and may not navigate away from the video: links open in the system browser. */
class WebEmbed : public QWidget {
    Q_OBJECT
public:
    WebEmbed(const QString &videoId, QWidget *parent);
signals:
    void closeRequested();

protected:
    void resizeEvent(QResizeEvent *) override;

private:
    QWebEngineView *view_;
    QToolButton *close_;
};

}

#endif
