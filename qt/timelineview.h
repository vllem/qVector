#ifndef VC_QT_TIMELINEVIEW_H
#define VC_QT_TIMELINEVIEW_H

#include <QElapsedTimer>
#include <QHash>
#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QPointer>
#include <QPushButton>
#include <QSet>
#include <QTextBrowser>
#include <QWidget>
#include "qt/videoplayer.h"

namespace vc {

/* The messages of the open room (or of one thread): a scrolling, selectable, dense IRC-style list drawn as rich text.
   The rows are what the engine sends (core-rs `UiMessage` as JSON); every user action is a signal carrying an event id. */
class TimelineView : public QWidget {
    Q_OBJECT
public:
    explicit TimelineView(bool thread = false, QWidget *parent = nullptr);
    ~TimelineView() override;
    void setMe(const QString &userId) { me_ = userId; }
    void setEncrypted(bool e) { encrypted_ = e; }
    /* a new room: forget the old rows and the scroll position */
    void reset();
    void setRows(const QJsonArray &rows);
    /* the link at the top: older messages can be loaded / are loading / there are none */
    void setHistoryState(bool canLoadMore, bool loading) { canLoadMore_ = canLoadMore; loadingHistory_ = loading; }
    void setBookmarks(const QSet<QString> &ids) { bookmarks_ = ids; }
    void refresh(); /* redraws; a view that is not on screen only notes that it is out of date and redraws when shown */
    void stickToBottom() { stick_ = true; }
    QString eventAt(const QPoint &pos) const; /* the message under a point of the view ("" for none) */
    void revealMessage(const QString &eventId);
    void playFile(const QString &eventId, const QString &path); /* plays a video message in place, over its card */
    void setPinned(const QSet<QString> &ids) { pinned_ = ids; }
    QJsonObject row(const QString &eventId) const;
    const QJsonArray &rows() const { return rows_; }

signals:
    void olderRequested();
    void openRequested(const QString &eventId);       /* open a picture/video/audio with the system player */
    void playRequested(const QString &eventId);       /* a video card was clicked: fetch it and call playFile */
    void replyRequested(const QString &eventId);
    void editRequested(const QString &eventId);
    void deleteRequested(const QString &eventId);
    void reactRequested(const QString &eventId, const QString &key);
    void reactPickerRequested(const QString &eventId);
    void saveRequested(const QString &eventId);
    void forwardRequested(const QString &eventId);
    void historyRequested(const QString &eventId);
    void threadRequested(const QString &rootEventId);
    void pinRequested(const QString &eventId, bool pin);
    void bookmarkRequested(const QString &eventId);
    void pollVote(const QString &eventId, const QString &answerId);
    void pollEnd(const QString &eventId);
    void matrixLink(const QString &href);

private:
    void onScroll();
    void applyStyle();
    void contextMenu(const QPoint &pos);
    void render();
    void stopInline();
    void placePlayer();
    void onAnchor(const QUrl &u);
    void showEvent(QShowEvent *) override;
    void hideEvent(QHideEvent *) override;
    void resizeEvent(QResizeEvent *) override;
    bool eventFilter(QObject *obj, QEvent *ev) override;
    int anchorY(const QString &eventId);
    const QImage *picture(const QString &path);

    bool thread_;
    QString me_, highlight_;
    bool encrypted_ = false, canLoadMore_ = false, loadingHistory_ = false;
    QJsonArray rows_;
    QTextBrowser *view_;
    bool stick_ = true, programmatic_ = false, stale_ = false, loadingMore_ = false;
    QString pendingReveal_, lastState_;
    quint64 resHash_ = 0, lastRes_ = 0;
    QHash<QUrl, QImage> resources_;
    QHash<QUrl, QSize> resSizes_;
    QHash<QString, QImage> pictures_;   /* decoded pictures by file path */
    QTextDocument *ownDoc_ = nullptr;
    size_t shownLimit_ = 100;
    QString windowStart_;
    QStringList lastChunks_, rowEvents_;
    QVector<int> chunkRowStart_;
    QString lastStyle_;
    QElapsedTimer lastRender_, userInput_;
    bool renderQueued_ = false;
    int hiddenLocal_ = 0, fitFetches_ = 0;
    QSet<QString> bookmarks_, pinned_;
    QString playingId_;
    QSize playingSize_;
    QPointer<VideoPlayer> player_;
    QString playingPath_;
};

}

#endif
