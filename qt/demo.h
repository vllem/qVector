#ifndef VC_QT_DEMO_H
#define VC_QT_DEMO_H

#include "qt/common.h"
#include <QStringList>

namespace vc {

/* A fake workspace for screenshots and for trying the UI without a server (--demo). Returns the rooms to open as tabs. */
QStringList loadDemo(VcModel *m, VcTransport *t, bool manyTabs);

}

#endif
