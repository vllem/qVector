#ifndef VC_QT_COMMON_H
#define VC_QT_COMMON_H

#include "qt/cbridge.h"
#include <QString>

namespace vc {

inline QString q(const char *s) { return QString::fromUtf8(s ? s : ""); }

/* the room's display name as the sidebar / tab / toolbar show it */
inline QString roomTitle(VcModel *m, const VcRoom *r)
{
    VcStr s;
    vc_str_init(&s);
    vc_room_display_name(r, m->session.user_id ? m->session.user_id : "", &s);
    if (r->is_invite && !r->name && !r->alias && r->inviter) { vc_str_clear(&s); vc_str_append(&s, r->inviter); }
    QString out = q(vc_str_cstr(&s));
    vc_str_free(&s);
    return out;
}

inline const VcEvent *findEvent(VcModel *m, const QString &room, const QString &event)
{
    VcRoom *r = vc_store_get(&m->store, room.toUtf8().constData());
    if (!r) return nullptr;
    size_t i = vc_room_find_event(r, event.toUtf8().constData());
    return i == size_t(-1) ? nullptr : vc_room_event(r, i);
}

}

#endif
