#ifndef VC_QT_UISCALE_H
#define VC_QT_UISCALE_H

#include <QFont>
#include <QObject>
#include <QSize>

class QAction;
class QWheelEvent;

namespace vc {

/* The size of the whole interface, in percent of what the desktop gives: View > Zoom in / Zoom out / Actual size (Ctrl++, Ctrl+-, Ctrl+0; Ctrl+wheel over
   the messages), remembered between runs. 100 is the desktop's own font and the sizes as they are written in the code.
   Two things follow it:
   * the application font (QApplication::setFont): the text of every widget that has no font of its own, the timeline's included;
   * every size the code writes as a number of pixels: px(24) is 24 at 100 % and 48 at 200 %. A widget that sets such sizes does it in a function that runs
     when the widget is made and again whenever the scale changes (onUiScale), so a dialog opened at 150 % has its 150 % sizes from the start. A font made by
     multiplying the application font's size (a larger title) is made in such a function too: a font with a size of its own does not follow the application font.
   What the style draws by itself (a scroll bar's width, a check box's size) stays as the theme has it. Lines of one pixel (dividers, frames) are not scaled. */

constexpr int kUiScaleMin = 50, kUiScaleMax = 300; /* percent */

int uiPercent();                    /* 100 = the desktop's size */
double uiScale();                   /* uiPercent() / 100 */
inline int px(int n) { return qRound(n * uiScale()); }
inline QSize px(int w, int h) { return QSize(px(w), px(h)); }
inline QSize px(const QSize &s) { return px(s.width(), s.height()); }
/* `f` with its size multiplied by k, whether the size is in points or in pixels */
QFont scaledFont(const QFont &f, double k);
/* The first size of a window that is w x h at 100 %: larger with a zoom above 100 %, but no larger than the screen holds (nine tenths of it), and never smaller
   than w x h (a zoom out makes the contents smaller, not the window). */
QSize pxFit(int w, int h);

/* Called once in main(), when the application font is the one the interface should start from (before any widget is made): the base the scale is applied to.
   percent <= 0: the one remembered from the last run; otherwise that percent (a dev aid; it is not remembered until it is changed). */
void initUiScale(int percent = 0);
/* Sets the scale (clamped to kUiScaleMin..kUiScaleMax), remembers it and applies it: the application font, then everything that listens (uiScaleNotifier). */
void setUiPercent(int percent);
void uiZoomIn();    /* the next size of the list 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300 */
void uiZoomOut();
void uiZoomReset(); /* 100 */
/* Ctrl+wheel: one step for each notch (a touch pad's small movements add up). True when `e` was a zoom (and is used up). */
bool uiZoomByWheel(const QWheelEvent *e);

class UiScaleNotifier : public QObject {
    Q_OBJECT
public:
    using QObject::QObject;
signals:
    void aboutToChange();      /* the scale is about to change: nothing has yet (a view notes the message it shows at the top) */
    void changed(int percent); /* it has: the application font and the font of every widget are the new ones */
};
UiScaleNotifier *uiScaleNotifier();

/* Runs `fn` now and again whenever the scale changes, for as long as `context` lives: the place for setFixedSize(px(28), px(28)) and the like. */
template <class F> inline void onUiScale(QObject *context, F fn)
{
    fn();
    QObject::connect(uiScaleNotifier(), &UiScaleNotifier::changed, context, [fn](int) { fn(); });
}

/* The View menu's three actions with their keys: Zoom in (Ctrl++ and Ctrl+=, which is the same key without Shift on most keyboards), Zoom out (Ctrl+-),
   Actual size (Ctrl+0). They are switched off at the ends of the range. The caller adds them to a menu and to its window. */
struct ZoomActions { QAction *in, *out, *reset; };
ZoomActions makeZoomActions(QObject *parent);

}

#endif
