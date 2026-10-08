#ifndef VC_QT_PACKPICKER_H
#define VC_QT_PACKPICKER_H

#include <QComboBox>
#include <QFrame>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QListWidget>

namespace vc {

/* Popup with the stickers of the packs that apply in the open room (the engine's `emote_packs` event). */
class PackPicker : public QFrame {
    Q_OBJECT
public:
    explicit PackPicker(QWidget *parent = nullptr);
    void setPacks(const QJsonArray &packs);
    void popupAt(const QPoint &anchorTopRight);
signals:
    void picked(const QJsonObject &emote); /* shortcode, mxc, width, height, mime: send it as a sticker */
private:
    void fill();
    QJsonArray packs_;
    QComboBox *pack_;
    QListWidget *grid_;
    QLabel *empty_;
};

}

#endif
