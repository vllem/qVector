#include "qt/uiscale.h"
#include "qt/theme.h"
#include <QAction>
#include <QApplication>
#include <QElapsedTimer>
#include <QFontInfo>
#include <QGuiApplication>
#include <QKeySequence>
#include <QLayout>
#include <QScreen>
#include <QSettings>
#include <QTimer>
#include <QWheelEvent>
#include <QWidget>
#include <iterator>

namespace vc {

namespace {
/* what Zoom in and Zoom out step through (the sizes a browser offers) */
constexpr int kSteps[] = {50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300};
static_assert(kSteps[0] == kUiScaleMin && kSteps[std::size(kSteps) - 1] == kUiScaleMax, "the steps span the whole range");

int g_percent = 100;
QFont g_base;       /* the desktop's application font: what 100 % is (with the emoji font behind it) */
QFont g_applied;    /* the application font as this file last made it (or found it): what a change by somebody else is told from */
bool g_haveBase = false, g_applying = false, g_checkQueued = false;

bool sameFont(const QFont &a, const QFont &b)
{
    return a.family() == b.family() && qFuzzyCompare(a.pointSizeF(), b.pointSizeF()) && a.pixelSize() == b.pixelSize() && a.weight() == b.weight() && a.style() == b.style();
}

void invalidateAll(QLayoutItem *item)
{
    item->invalidate();
    if (QLayout *l = item->layout()) for (int i = 0; QLayoutItem *child = l->itemAt(i); i++) invalidateAll(child);
}

/* the application font at the current scale; QApplication::setFont() also drops the fonts a desktop theme gave to single kinds of widgets (menus, the status
   bar, tool buttons), so that every widget has this one */
void apply()
{
    g_applying = true;
    g_applied = scaledFont(g_base, uiScale());
    QApplication::setFont(g_applied);
    /* Qt tells the widgets of it a moment later, from the event loop, and until then a widget still has its old font: a height worked out from its font metrics
       by whoever listens for the change would be the old one. The news goes to every widget now (each one takes the application font: the ones below a style
       sheet do not get their font from their parent, so the windows alone are not enough); the second time it comes, from Qt, it changes nothing. */
    QEvent e(QEvent::ApplicationFontChange);
    const QWidgetList all = QApplication::allWidgets();
    for (QWidget *w : all) QCoreApplication::sendEvent(w, &e);
    /* A widget that is hidden does not tell its parent's layout that its size has changed: the layout of the sign-in page, of a panel that is closed, keeps the
       sizes of the old font until something else invalidates it, and the page or panel opened later is laid out with them (and the window's minimum size is
       taken from them in the meantime). The layouts inside layouts are cleared too: a layout only does that to those below it when it is activated. */
    for (QWidget *w : all) if (QLayout *l = w->layout()) invalidateAll(l);
    g_applying = false;
}

/* The desktop's theme (qt6ct, KDE's, ...) sets the application font by itself when the event loop starts and whenever its settings or the theme change, and
   gives some kinds of widgets fonts of their own again: that would undo the zoom for some widgets (the menu bar, the status bar) and for every widget made
   afterwards. This puts it back. When the font that was set is not the one this file made and not the old desktop font either, the desktop has a new font:
   that is the new 100 %. At 100 % nothing is touched (the desktop's choices stand). */
void checkApplicationFont()
{
    g_checkQueued = false;
    const QFont now = QApplication::font();
    bool newBase = false;
    if (!sameFont(now, g_applied)) {
        if (!sameFont(now, g_base)) { g_base = withEmojiFallback(now); newBase = true; }
        g_applied = now;
    }
    if (g_percent != 100) apply();
    if (newBase) emit uiScaleNotifier()->changed(g_percent); /* what is made from the font (a title a half larger, a height of two text lines) is made again */
}

class FontWatcher : public QObject {
public:
    using QObject::QObject;
    bool eventFilter(QObject *o, QEvent *e) override
    {
        if (o != QCoreApplication::instance() || g_applying || g_checkQueued) return false;
        if (e->type() != QEvent::ApplicationFontChange && e->type() != QEvent::ThemeChange) return false;
        g_checkQueued = true; /* later: a theme sets its font and then its widget fonts (the ThemeChange); the check comes after both have been handled */
        QTimer::singleShot(0, qApp, [] { checkApplicationFont(); });
        if (g_percent != 100) QTimer::singleShot(400, qApp, [] { if (g_percent != 100 && !g_checkQueued) checkApplicationFont(); }); /* and once more, for a theme that is slower */
        return false;
    }
};

void ensureBase()
{
    if (g_haveBase) return;
    g_base = QApplication::font();
    g_applied = g_base;
    g_haveBase = true;
    qApp->installEventFilter(new FontWatcher(qApp));
}
}

int uiPercent() { return g_percent; }
double uiScale() { return g_percent / 100.0; }

QFont scaledFont(const QFont &f, double k)
{
    QFont out(f);
    if (f.pixelSize() > 0) out.setPixelSize(qMax(1, qRound(f.pixelSize() * k)));
    else if (f.pointSizeF() > 0) out.setPointSizeF(f.pointSizeF() * k);
    return out;
}

QSize pxFit(int w, int h)
{
    const QSize at100(w, h);
    if (g_percent <= 100) return at100;
    QSize want = px(at100);
    if (const QScreen *screen = QGuiApplication::primaryScreen()) want = want.boundedTo(screen->availableGeometry().size() * 0.9);
    return want.expandedTo(at100);
}

UiScaleNotifier *uiScaleNotifier()
{
    static UiScaleNotifier *n = new UiScaleNotifier(QCoreApplication::instance()); /* it lives as long as the application */
    return n;
}

void initUiScale(int percent)
{
    ensureBase();
    if (percent <= 0) {
        bool ok = false;
        percent = QSettings().value("ui_scale", 100).toInt(&ok);
        if (!ok) percent = 100;
    }
    g_percent = qBound(kUiScaleMin, percent, kUiScaleMax);
    if (g_percent != 100) apply();
}

void setUiPercent(int percent)
{
    ensureBase();
    percent = qBound(kUiScaleMin, percent, kUiScaleMax);
    if (percent == g_percent) return;
    UiScaleNotifier *n = uiScaleNotifier();
    emit n->aboutToChange();
    g_percent = percent;
    apply(); /* every widget has the new font when this returns */
    QSettings().setValue("ui_scale", percent);
    emit n->changed(percent);
}

void uiZoomIn()
{
    for (int s : kSteps) if (s > g_percent) { setUiPercent(s); return; }
}

void uiZoomOut()
{
    for (int i = int(std::size(kSteps)) - 1; i >= 0; i--) if (kSteps[i] < g_percent) { setUiPercent(kSteps[i]); return; }
}

void uiZoomReset() { setUiPercent(100); }

bool uiZoomByWheel(const QWheelEvent *e)
{
    if (!(e->modifiers() & Qt::ControlModifier)) return false;
    static int rest = 0; /* movement of the wheel that does not make a step yet: a touch pad sends many small ones */
    static QElapsedTimer last;
    if (!last.isValid() || last.elapsed() > 700) rest = 0;
    last.restart();
    rest += e->angleDelta().y();
    while (rest >= 120) { rest -= 120; uiZoomIn(); }
    while (rest <= -120) { rest += 120; uiZoomOut(); }
    return true;
}

ZoomActions makeZoomActions(QObject *parent)
{
    auto *in = new QAction("Zoom in", parent), *out = new QAction("Zoom out", parent), *reset = new QAction("Actual size", parent);
    /* Ctrl++ is Ctrl+Shift+= on a keyboard with + above =, and Qt's standard key is only the one written with the +: Ctrl+= is listed as well */
    QList<QKeySequence> keys = QKeySequence::keyBindings(QKeySequence::ZoomIn);
    for (const char *extra : {"Ctrl+=", "Ctrl+Shift++"}) if (!keys.contains(QKeySequence(extra))) keys.append(QKeySequence(extra));
    in->setShortcuts(keys);
    out->setShortcuts(QKeySequence::keyBindings(QKeySequence::ZoomOut));
    reset->setShortcut(QKeySequence("Ctrl+0"));
    QObject::connect(in, &QAction::triggered, in, [] { uiZoomIn(); });
    QObject::connect(out, &QAction::triggered, out, [] { uiZoomOut(); });
    QObject::connect(reset, &QAction::triggered, reset, [] { uiZoomReset(); });
    auto update = [in, out, reset](int percent) {
        in->setEnabled(percent < kUiScaleMax);
        out->setEnabled(percent > kUiScaleMin);
        reset->setEnabled(percent != 100);
    };
    update(uiPercent());
    QObject::connect(uiScaleNotifier(), &UiScaleNotifier::changed, in, update);
    return {in, out, reset};
}

}
