#ifndef VC_QT_CBRIDGE_H
#define VC_QT_CBRIDGE_H

/* The C core and model, for C++ code. The headers are plain C; everything here is declared extern "C". */
extern "C" {
#include "app/htmlfmt.h"
#include "app/images.h"
#include "app/links.h"
#include "app/model.h"
#include "matrix/e2ee.h"
#include "matrix/media.h"
#include "matrix/room.h"
#include "net/transport.h"
}

#endif
