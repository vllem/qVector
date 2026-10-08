#ifndef VC_QT_EMOJIPICKER_H
#define VC_QT_EMOJIPICKER_H

#include "qt/emoji.h"
#include <QFrame>
#include <QJsonArray>
#include <QJsonObject>
#include <QLineEdit>
#include <QListWidget>
#include <QToolButton>

namespace vc {

/* Popup with every emoji: search, categories, recently used. */
class EmojiPicker : public QFrame {
    Q_OBJECT
public:
    explicit EmojiPicker(QWidget *parent = nullptr);
    void popupAt(const QPoint &globalBottomLeft);
    void setCloseOnPick(bool on) { closeOnPick_ = on; }
    void setCustom(const QJsonArray &packs); /* the room's custom emoji (engine `emote_packs`): one more category */
signals:
    void emojiPicked(const QString &glyph);
    void customPicked(const QString &shortcode); /* a custom emoji: insert :shortcode: */
private:
    void fill();
    QLineEdit *search_;
    QList<QToolButton *> cats_;   /* category buttons, all always visible: recent and the nine groups */
    int cat_ = 0;
    void setCategory(int i);
    QListWidget *grid_;
    bool closeOnPick_ = false;
    QList<QJsonObject> custom_;
    QToolButton *customBtn_ = nullptr;
};

}

#endif
