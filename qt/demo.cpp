#include "qt/demo.h"
#include "qt/demo_data.h"
#include <QBuffer>
#include <QImage>
#include <QJsonArray>
#include <QJsonDocument>
#include <QPainter>
#include <QJsonObject>
#include <cstdlib>
#include <string>
#include <ctime>
#include <cstring>

namespace vc {

static JsonValue *parse(const QJsonObject &o)
{
    QByteArray b = QJsonDocument(o).toJson(QJsonDocument::Compact);
    return json_parse(b.constData(), size_t(b.size()), nullptr);
}

static void member(VcRoom *r, const QString &user, const QString &name, const QString &avatar = QString())
{
    QJsonObject c{{"membership", "join"}, {"displayname", name}};
    if (!avatar.isEmpty()) c.insert("avatar_url", avatar);
    vc_room_apply_state(r, parse({{"type", "m.room.member"}, {"state_key", user}, {"sender", user}, {"content", c}}));
}

static void state(VcRoom *r, const char *type, const char *key, const QString &val)
{
    vc_room_apply_state(r, parse({{"type", type}, {"state_key", ""}, {"sender", "@x:example.org"}, {"content", QJsonObject{{key, val}}}}));
}

static VcRoom *mkroom(VcModel *m, const QString &id, const QString &name, const QString &topic, int unread)
{
    VcRoom *r = vc_store_get_or_create(&m->store, id.toUtf8().constData());
    if (name == "cmake" || name == "qt" || name == "msvc") state(r, "m.room.avatar", "url", "mxc://example.org/room-" + name);
    if (!name.isEmpty()) state(r, "m.room.name", "name", name);
    if (!topic.isEmpty()) state(r, "m.room.topic", "topic", topic);
    member(r, "@cancel:example.org", "cancel");
    r->unread = unread;
    return r;
}

static int g_n = 0;
static void msg(VcRoom *r, const QString &user, qint64 ts, const QString &body)
{
    vc_room_add_timeline_event(r, parse({{"event_id", QString("$m%1").arg(++g_n)}, {"sender", user}, {"type", "m.room.message"}, {"origin_server_ts", ts},
                                         {"content", QJsonObject{{"msgtype", "m.text"}, {"body", body}}}}));
}

static void preload(VcModel *m, const char *key, const unsigned char *data, size_t len, int w, int h, int file)
{
    VcImage *im = static_cast<VcImage *>(calloc(1, sizeof *im));
    if (!im || m->images.n >= VC_IMAGE_CACHE_MAX) { free(im); return; }
    im->id = ++m->images.next_id;
    snprintf(im->key, sizeof im->key, "%s", key);
    im->bytes = static_cast<uint8_t *>(malloc(len));
    if (!im->bytes) { free(im); return; }
    memcpy(im->bytes, data, len);
    im->len = len; im->w = w; im->h = h; im->state = VC_IMG_READY; im->started = 1; im->is_file = file;
    m->images.bytes += len;
    m->images.v[m->images.n++] = im;
}

/* a made-up profile picture, handed to the image cache as if the server had delivered it */
static void demoAvatar(VcModel *m, const QString &mxc, const QColor &bg, const QString &glyph)
{
    QImage img(96, 96, QImage::Format_ARGB32);
    img.fill(bg);
    QPainter p(&img);
    p.setRenderHint(QPainter::Antialiasing);
    p.setBrush(bg.lighter(150)); p.setPen(Qt::NoPen);
    p.drawEllipse(QPointF(48, 40), 20, 20);
    p.drawEllipse(QRectF(16, 64, 64, 60));
    p.setPen(Qt::white);
    QFont f = p.font(); f.setPixelSize(22); f.setBold(true); p.setFont(f);
    p.drawText(QRect(0, 0, 96, 96), Qt::AlignBottom | Qt::AlignHCenter, glyph);
    p.end();
    QByteArray png;
    QBuffer b(&png);
    b.open(QIODevice::WriteOnly);
    img.save(&b, "PNG");
    QByteArray key = ("av:" + mxc).toUtf8();
    preload(m, key.constData(), reinterpret_cast<const unsigned char *>(png.constData()), size_t(png.size()), 96, 96, 0);
    if (mxc.contains("emoji")) { /* custom emoji are looked up as 64x64 thumbnails */
        key = ("th64x64:" + mxc).toUtf8();
        preload(m, key.constData(), reinterpret_cast<const unsigned char *>(png.constData()), size_t(png.size()), 96, 96, 0);
    }
}

static void demoPresence(VcModel *m, const char *user, const char *state, int64_t agoMs, const char *status = "")
{
    VcPresence *p = static_cast<VcPresence *>(calloc(1, sizeof *p));
    if (!p) return;
    snprintf(p->state, sizeof p->state, "%s", state);
    snprintf(p->status_msg, sizeof p->status_msg, "%s", status);
    p->last_active_ms = int64_t(time(nullptr)) * 1000 - agoMs;
    if (VcPresenceMap_set(&m->presence, user, p) != VC_OK) free(p);
}

QStringList loadDemo(VcModel *m, VcTransport *t, bool manyTabs)
{
    struct Chan { const char *name; int unread; const char *topic; };
    static const Chan chans[] = {{"announcements", 0, ""}, {"build_systems", 0, ""}, {"cmake", 1, ""},
                                 {"compiler_explorer", 0, "Discussion of https://godbolt.org/ - beta at https://godbolt.org/beta/"},
                                 {"general", 2, ""}, {"meta-programming", 1, ""}, {"msvc", 0, "Visual Studio, MSBuild, Windows SDK's"}, {"qt", 0, ""},
                                 {"standardese", 1, ""}, {"stl", 1, ""}};
    static const char *dms[][2] = {{"Dimitar Dobrev", "@dimitar:example.org"}, {"Q.", "@q:example.org"}, {"Astrid", "@astrid:example.org"}, {"kma", "@kma:example.org"},
                                   {"tmad40blue", "@tmad:example.org"}, {"Slackbot", "@slackbot:example.org"}, {"A_Rival", "@arival:example.org"}};
    const qint64 t0 = 1568500000000LL;
    VcRoom *msvc = nullptr, *arival = nullptr;
    vc_session_init(&m->session, t, "https://example.org");
    vc_session_restore(&m->session, "demo", "@cancel:example.org", "DEMO");
    int i = 0;
    for (const Chan &c : chans) {
        VcRoom *r = mkroom(m, QString("!%1:example.org").arg(c.name), c.name, c.topic, c.unread);
        r->last_ts = t0 - qint64(i++) * 1000;
        if (!strcmp(c.name, "msvc")) msvc = r;
    }
    i = 0;
    for (auto &d : dms) {
        VcRoom *r = mkroom(m, QString("!dm%1:example.org").arg(i), QString(), QString(), 0);
        member(r, d[1], d[0], !strcmp(d[0], "Astrid") ? QString("mxc://example.org/astrid") : QString());
        r->last_ts = t0 - 100000 - qint64(i++) * 1000;
        if (!strcmp(d[0], "A_Rival")) arival = r;
    }
    {   /* a space listing three of the channels */
        VcRoom *space = mkroom(m, "!space-team:example.org", "The Team", "Everything the team talks about", 0);
        vc_room_apply_state(space, parse({{"type", "m.room.create"}, {"state_key", ""}, {"sender", "@cancel:example.org"}, {"content", QJsonObject{{"type", "m.space"}}}}));
        for (const char *c : {"cmake", "qt", "msvc", "stl"})
            vc_room_apply_state(space, parse({{"type", "m.space.child"}, {"state_key", QString("!%1:example.org").arg(c)}, {"sender", "@cancel:example.org"}, {"content", QJsonObject{{"via", QJsonArray{"example.org"}}}}}));
        space->last_ts = t0 - 900000;
    }
    VcRoom *lobby = mkroom(m, "!the-lobby:example.org", "the-lobby", "Say hello", 0);
    lobby->last_ts = t0 - 500000;
    member(msvc, "@zach:example.org", "Zachary Turner", "mxc://example.org/zach");
    member(msvc, "@ben:example.org", "ben.craig");
    member(msvc, "@matt:example.org", "matt", "mxc://example.org/matt");
    member(msvc, "@kab:example.org", "kaboissonneault");
    member(msvc, "@ruben:example.org", QString::fromUtf8("Rub\xc3\xa9n"));
    vc_room_apply_state(msvc, parse({{"type", "m.room.create"}, {"state_key", ""}, {"sender", "@cancel:example.org"}, {"content", QJsonObject{{"room_version", "11"}}}}));
    vc_room_apply_state(msvc, parse({{"type", "m.room.power_levels"}, {"state_key", ""}, {"sender", "@cancel:example.org"},
        {"content", QJsonObject{{"users", QJsonObject{{"@cancel:example.org", 100}, {"@zach:example.org", 50}}}}}}));
    { VcBookmark *bm = static_cast<VcBookmark *>(calloc(VC_BOOKMARKS_MAX, sizeof *bm)); if (bm) { m->bookmarks = bm; m->nbookmarks = 1;
        snprintf(bm->room_id, sizeof bm->room_id, "!msvc:example.org"); snprintf(bm->event_id, sizeof bm->event_id, "$fmt1"); snprintf(bm->sender, sizeof bm->sender, "ben.craig");
        snprintf(bm->text, sizeof bm->text, "Build steps: configure, build, test. Use cmake --build."); bm->ts = t0 + 4380000; } }
    { /* embeds: a YouTube video and a post, already "fetched" */
        m->embeds_enabled = 1;
        auto addEmbed = [m](const char *url, VcEmbedKind kind, const char *id, const char *title, const char *author, const char *text, const char *byline) {
            VcEmbedInfo *e = static_cast<VcEmbedInfo *>(calloc(1, sizeof *e));
            if (!e || m->nembeds >= VC_EMBED_MAX) { free(e); return; }
            snprintf(e->url, sizeof e->url, "%s", url); e->kind = kind; snprintf(e->id, sizeof e->id, "%s", id);
            snprintf(e->title, sizeof e->title, "%s", title); snprintf(e->author, sizeof e->author, "%s", author);
            snprintf(e->text, sizeof e->text, "%s", text); snprintf(e->byline, sizeof e->byline, "%s", byline);
            vc_embed_page_url(kind, id, e->page, sizeof e->page);
            if (kind == VC_EMBED_YOUTUBE) vc_embed_youtube_thumbnail(id, e->thumb, sizeof e->thumb);
            e->state = VC_EMBED_READY;
            m->embeds[m->nembeds++] = e;
        };
        addEmbed("https://youtu.be/dQw4w9WgXcQ", VC_EMBED_YOUTUBE, "dQw4w9WgXcQ", "Rick Astley - Never Gonna Give You Up (Official Video)", "Rick Astley", "", "");
        { /* the video's picture, drawn here instead of fetched */
            QImage img(480, 360, QImage::Format_ARGB32);
            QLinearGradient g(0, 0, 480, 360);
            g.setColorAt(0, QColor(0x2b, 0x1d, 0x5c)); g.setColorAt(1, QColor(0xc0, 0x3a, 0x6a));
            QPainter p(&img);
            p.fillRect(img.rect(), g);
            p.setPen(Qt::white);
            QFont f = p.font(); f.setPixelSize(40); f.setBold(true); p.setFont(f);
            p.drawText(img.rect(), Qt::AlignCenter, "Never Gonna\nGive You Up");
            p.end();
            QByteArray png;
            QBuffer b(&png);
            b.open(QIODevice::WriteOnly);
            img.save(&b, "PNG");
            const char *thumbUrl = "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg";
            uint32_t h = 2166136261u;
            for (const char *s = thumbUrl; *s; s++) h = (h ^ uint8_t(*s)) * 16777619u; /* the image cache's key for an address (see vc_images_get_external) */
            const QByteArray key = QString("ext:%1:%2").arg(h, 8, 16, QChar('0')).arg(thumbUrl).toUtf8();
            preload(m, key.constData(), reinterpret_cast<const unsigned char *>(png.constData()), size_t(png.size()), 480, 360, 0);
        }
        addEmbed("https://twitter.com/jack/status/20", VC_EMBED_TWEET, "jack/20", "", "jack", "just setting up my twttr", "\xE2\x80\x94 jack (@jack) March 21, 2006");
    }
    msvc->favourite = 1; msvc->fav_order = 0.5;
    if (VcRoom *stl = vc_store_get(&m->store, "!stl:example.org")) stl->lowpriority = 1;
    { const char *pr = "{\"global\":{\"override\":[{\"rule_id\":\"!qt:example.org\",\"enabled\":true,\"actions\":[],\"conditions\":[]}],\"room\":[]}}"; m->push_rules = json_parse(pr, strlen(pr), nullptr); }
    vc_room_apply_state(msvc, parse({{"type", "m.room.pinned_events"}, {"state_key", ""}, {"sender", "@cancel:example.org"},
        {"content", QJsonObject{{"pinned", QJsonArray{"$fmt1", "$m3"}}}}}));
    vc_room_apply_state(msvc, parse({{"type", "m.room.member"}, {"state_key", "@spammer:example.org"}, {"sender", "@cancel:example.org"},
        {"content", QJsonObject{{"membership", "ban"}, {"displayname", "spammer"}}}}));
    if (const char *nr = getenv("VC_DEMO_ROOMS")) for (int k = 0; k < atoi(nr); k++) { /* many rooms, for measuring */
        VcRoom *x = mkroom(m, QString("!bulk%1:example.org").arg(k), QString("bulk room %1").arg(k), "", k % 5);
        for (int j = 0; j < 20; j++) msg(x, "@zach:example.org", t0 + j * 1000, "hello");
    }
    if (getenv("VC_DEMO_BIG")) for (int k = 0; k < 480; k++) { /* a long room, for measuring */
        static const char *const who[] = {"@zach:example.org", "@ben:example.org", "@matt:example.org", "@ruben:example.org"};
        msg(msvc, who[k % 4], t0 - 1000000 + k * 60000, k % 7 == 3 ? QString("link %1: https://www.example.org/a/very/long/path/with/many/segments/and/no/spaces/at/all/so/it/cannot/wrap/at/a/word/boundary/at/all?query=1&other=2&more=3&evenmore=4&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").arg(k) : QString("message %1: the quick brown fox jumps over the lazy dog, again and again until the line wraps around the window width").arg(k));
    }
    msg(msvc, "@zach:example.org", t0 + 1000, "Yes but otoh it seems impossible for msvc to fix this");
    msg(msvc, "@ben:example.org", t0 + 2000, QString::fromUtf8("Things I learn: You can pass /delayload:foo.dll to the MSVC 2015 linker without passing foo.lib on the link line.  If you do that, you'll get warnings about /delayload being dropped before you get errors about unresolved symbols from the missing foo.lib.  If I had gotten the unresolved symbol error first, it would have been a 2 minute fix.  Spent several hours tracking down the issue instead. \xf0\x9f\x98\x82"));
    msg(msvc, "@matt:example.org", t0 + 3000, "https://twitter.com/dvyukov/status/1173669318971838464");
    msg(msvc, "@kab:example.org", t0 + 4000, QString::fromUtf8("asan for MSVC confirmed?! \xf0\x9f\x8e\xb5"));
    msg(msvc, "@matt:example.org", t0 + 4060000, "Sounds like it, yeah!");
    msg(msvc, "@matt:example.org", t0 + 4070000, "Open Sourcing MSVC's STL\nhttps://devblogs.microsoft.com/cppblog/open-sourcing-msvcs-stl/\nMSVC's implementation of the C++ Standard Library.\nhttps://github.com/microsoft/STL");
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$img1"}, {"sender", "@ruben:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", 1568504200000LL},
        {"content", QJsonObject{{"msgtype", "m.image"}, {"body", "sunset.png"}, {"url", "mxc://example.org/sunset"}, {"info", QJsonObject{{"w", 480}, {"h", 300}, {"mimetype", "image/png"}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$vid1"}, {"sender", "@matt:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", 1568504300000LL},
        {"content", QJsonObject{{"msgtype", "m.video"}, {"body", "clip.mp4"}, {"url", "mxc://example.org/clip"},
                                {"info", QJsonObject{{"w", 96}, {"h", 64}, {"duration", 1200}, {"size", 7652}, {"thumbnail_url", "mxc://example.org/clipthumb"}}}}}}));
    for (auto *who : {"@zach:example.org", "@ben:example.org", "@cancel:example.org"})
        vc_room_add_timeline_event(msvc, parse({{"event_id", QString("$rx-") + who}, {"sender", who}, {"type", "m.reaction"}, {"origin_server_ts", t0 + 4500000},
            {"content", QJsonObject{{"m.relates_to", QJsonObject{{"rel_type", "m.annotation"}, {"event_id", "$reply1"}, {"key", QString::fromUtf8("\xf0\x9f\x91\x8d")}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$rx2"}, {"sender", "@ben:example.org"}, {"type", "m.reaction"}, {"origin_server_ts", t0 + 4500000},
        {"content", QJsonObject{{"m.relates_to", QJsonObject{{"rel_type", "m.annotation"}, {"event_id", "$reply1"}, {"key", QString::fromUtf8("\xe2\x9d\xa4\xef\xb8\x8f")}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$reply1"}, {"sender", "@kab:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4600000},
        {"content", QJsonObject{{"msgtype", "m.text"}, {"body", "Agreed, that works for me."}, {"m.relates_to", QJsonObject{{"m.in_reply_to", QJsonObject{{"event_id", "$m3"}}}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$file1"}, {"sender", "@zach:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4350000},
        {"content", QJsonObject{{"msgtype", "m.file"}, {"body", "build-report.pdf"}, {"url", "mxc://example.org/report"}, {"info", QJsonObject{{"size", 48210}, {"mimetype", "application/pdf"}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$fmt1"}, {"sender", "@ben:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4380000},
        {"content", QJsonObject{{"msgtype", "m.text"}, {"body", "Build steps: configure, build, test. Use cmake --build. See https://cmake.org"},
            {"format", "org.matrix.custom.html"},
            {"formatted_body", "<p><strong>Build steps:</strong> <em>configure</em>, <u>build</u>, <del>skip</del> test.</p><ol><li>cmake -S . -B build</li><li>cmake --build build</li></ol>"
                               "<blockquote>Tip: use <code>-j8</code></blockquote><pre><code>ctest --output-on-failure\n</code></pre>See https://cmake.org <a href=\"https://matrix.to/#/@zach:example.org\">Zachary Turner</a> "
                               "<script>alert(1)</script><span data-mx-color=\"#d9534f\">red text</span> nice <img data-mx-emoticon src=\"mxc://example.org/emoji-cat\" alt=\":cat:\" height=\"32\"> <img src=\"https://tracker.example/p.png\" alt=\"[tracker]\">"}}}}));
    for (int k = 0; k < 2; k++)
        vc_room_add_timeline_event(msvc, parse({{"event_id", QString("$th%1").arg(k)}, {"sender", k ? "@ben:example.org" : "@zach:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4700000 + k * 60000},
            {"content", QJsonObject{{"msgtype", "m.text"}, {"body", k ? "Yes, it landed in the nightly build." : "Does that fix the asan crash on Windows?"},
                                    {"m.relates_to", QJsonObject{{"rel_type", "m.thread"}, {"event_id", "$m4"}, {"is_falling_back", true}, {"m.in_reply_to", QJsonObject{{"event_id", k ? "$th0" : "$m4"}}}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$poll1"}, {"sender", "@zach:example.org"}, {"type", "org.matrix.msc3381.poll.start"}, {"origin_server_ts", t0 + 4390000},
        {"content", QJsonObject{{"org.matrix.msc3381.poll.start", QJsonObject{{"kind", "org.matrix.msc3381.poll.disclosed"}, {"max_selections", 1},
            {"question", QJsonObject{{"org.matrix.msc1767.text", "Which compiler for the next CI job?"}}},
            {"answers", QJsonArray{QJsonObject{{"id", "a"}, {"org.matrix.msc1767.text", "MSVC 2022"}}, QJsonObject{{"id", "b"}, {"org.matrix.msc1767.text", "clang-cl"}}, QJsonObject{{"id", "c"}, {"org.matrix.msc1767.text", "Both"}}}}}}}}}));
    for (int k = 0; k < 4; k++)
        vc_room_add_timeline_event(msvc, parse({{"event_id", QString("$pv%1").arg(k)}, {"sender", k == 3 ? "@cancel:example.org" : QString("@u%1:example.org").arg(k)}, {"type", "org.matrix.msc3381.poll.response"}, {"origin_server_ts", t0 + 4391000 + k},
            {"content", QJsonObject{{"m.relates_to", QJsonObject{{"rel_type", "m.reference"}, {"event_id", "$poll1"}}}, {"org.matrix.msc3381.poll.response", QJsonObject{{"answers", QJsonArray{k == 0 ? "a" : "b"}}}}}}}));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$yt1"}, {"sender", "@matt:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4395000},
        {"content", QJsonObject{{"msgtype", "m.text"}, {"body", "this one is a classic https://youtu.be/dQw4w9WgXcQ"}}}}));
    msg(msvc, "@zach:example.org", t0 + 4396000, "and the first post ever: https://twitter.com/jack/status/20");
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$sp1"}, {"sender", "@ben:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4398000},
        {"content", QJsonObject{{"msgtype", "m.text"}, {"body", "The answer is 42"}, {"format", "org.matrix.custom.html"}, {"formatted_body", "The answer is <span data-mx-spoiler=\"\">42</span>, said the <span data-mx-spoiler>mouse</span>."}}}}));
    msg(msvc, "@zach:example.org", t0 + 4399000, QString::fromUtf8("\xF0\x9F\x98\x82\xF0\x9F\x91\x8D\xF0\x9F\x87\xAA\xF0\x9F\x87\xAA"));
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$loc1"}, {"sender", "@matt:example.org"}, {"type", "m.room.message"}, {"origin_server_ts", t0 + 4399500},
        {"content", QJsonObject{{"msgtype", "m.location"}, {"body", "Location: Tallinn Old Town"}, {"geo_uri", "geo:59.4370,24.7536"}}}}));
    msg(msvc, "@ruben:example.org", t0 + 4400000, "A fix is now almost ready but it might take a bit to hit the live site, as it needs a review :)");
    msvc->last_ts = t0 + 4400000;
    msg(arival, "@arival:example.org", t0, "Did you see the new release?");
    demoAvatar(m, "mxc://example.org/emoji-cat", QColor(0xe0, 0xa0, 0x30), "=^");
    demoAvatar(m, "mxc://example.org/zach", QColor(0xb0, 0x40, 0x40), "ZT");
    demoAvatar(m, "mxc://example.org/matt", QColor(0x30, 0x70, 0xb0), "m");
    demoAvatar(m, "mxc://example.org/astrid", QColor(0x40, 0x90, 0x60), "A");
    demoAvatar(m, "mxc://example.org/room-cmake", QColor(0x2a, 0x7a, 0x3a), "cm");
    demoAvatar(m, "mxc://example.org/room-qt", QColor(0x30, 0xa0, 0x60), "Qt");
    demoAvatar(m, "mxc://example.org/room-msvc", QColor(0x60, 0x40, 0xa0), "VS");
    {   /* a link preview, as if the server had answered */
        VcPreview *pv = static_cast<VcPreview *>(calloc(1, sizeof *pv));
        snprintf(pv->url, sizeof pv->url, "https://cmake.org");
        pv->state = VC_PREVIEW_READY;
        snprintf(pv->title, sizeof pv->title, "CMake");
        snprintf(pv->site, sizeof pv->site, "cmake.org");
        snprintf(pv->desc, sizeof pv->desc, "CMake is an open-source, cross-platform family of tools designed to build, test and package software.");
        snprintf(pv->image, sizeof pv->image, "mxc://example.org/cmakeimg");
        m->previews[m->npreviews++] = pv;
        preload(m, "th160x120:mxc://example.org/cmakeimg", DEMO_PNG, sizeof DEMO_PNG, 480, 300, 0);
    }
    vc_room_add_timeline_event(msvc, parse({{"event_id", "$sticker1"}, {"sender", "@matt:example.org"}, {"type", "m.sticker"}, {"origin_server_ts", t0 + 4800000},
        {"content", QJsonObject{{"body", "Sunset sticker"}, {"url", "mxc://example.org/sunset"}, {"info", QJsonObject{{"w", 128}, {"h", 80}, {"mimetype", "image/png"}}}}}}));
    preload(m, "$sticker1#inline", DEMO_PNG, sizeof DEMO_PNG, 128, 80, 0);
    preload(m, "$img1#inline", DEMO_PNG, sizeof DEMO_PNG, 480, 300, 0);
    preload(m, "$img1#full", DEMO_PNG, sizeof DEMO_PNG, 480, 300, 0);
    preload(m, "$vid1#vthumb", DEMO_PNG, sizeof DEMO_PNG, 480, 300, 0);
    preload(m, "$vid1#file", DEMO_MP4, sizeof DEMO_MP4, 0, 0, 1);
    demoPresence(m, "@astrid:example.org", "online", 1000, "in a meeting");
    demoPresence(m, "@dimitar:example.org", "unavailable", 600000);
    demoPresence(m, "@q:example.org", "offline", 7200000);
    demoPresence(m, "@zach:example.org", "online", 500);
    demoPresence(m, "@ben:example.org", "offline", 90000);
    demoPresence(m, "@matt:example.org", "unavailable", 200000);
    { static const char *typers = "[\"@zach:example.org\",\"@ben:example.org\"]"; JsonValue *ids = json_parse(typers, strlen(typers), nullptr); vc_room_set_typing(msvc, ids); json_free(ids); }
#ifndef VC_RUST_CRYPTO /* the Rust engine has no trust data to fake yet */
    {   /* trust: Zachary is verified (one verified session, one not), ben.craig's identity changed, matt is just unverified */
        m->e2ee = static_cast<VcE2ee *>(calloc(1, sizeof *m->e2ee));
        if (m->e2ee && vc_e2ee_init(m->e2ee, &m->session, &m->store, vc_crypto_openssl()) == VC_OK) {
            auto user = [&](const char *id, const char *master, const char *pinned, const char *verified) {
                vc_e2ee_track_user(m->e2ee, id);
                VcUserKeys *u = vc_e2ee_user_keys(m->e2ee, id);
                snprintf(u->master_pub, sizeof u->master_pub, "%s", master);
                snprintf(u->pinned_master, sizeof u->pinned_master, "%s", pinned);
                snprintf(u->verified_master, sizeof u->verified_master, "%s", verified);
                u->fetched = 1; u->want_fetch = 0;
            };
            user("@zach:example.org", "ZM", "ZM", "ZM");
            user("@ben:example.org", "NEW", "OLD", "OLD");
            user("@matt:example.org", "MM", "MM", "");
            vc_e2ee_trust_refresh(m->e2ee);
            auto device = [&](const char *u, const char *dev, int ok) {
                VcDevice *d = static_cast<VcDevice *>(calloc(1, sizeof *d));
                d->user_id = strdup(u); d->device_id = strdup(dev);
                snprintf(d->curve, sizeof d->curve, "%s-key", dev);
                d->verified = ok;
                m->e2ee->devices = static_cast<VcDevice **>(realloc(m->e2ee->devices, (m->e2ee->ndevices + 1) * sizeof(VcDevice *)));
                m->e2ee->devices[m->e2ee->ndevices++] = d;
            };
            device("@zach:example.org", "ZGOOD", 1);
            device("@zach:example.org", "ZOLD", 0);
            msvc->encrypted = 1;
            for (size_t k = 0; k < vc_room_event_count(msvc); k++) {
                VcEvent *ev = vc_room_event(msvc, k);
                if (!ev->sender || strcmp(ev->type, "m.room.message") != 0) continue;
                const char *dev = !strcmp(ev->sender, "@zach:example.org") ? (k % 2 ? "ZGOOD" : "ZOLD") : "XD";
                ev->enc_device = strdup(dev);
                ev->enc_key = strdup((std::string(dev) + "-key").c_str());
            }
        }
    }
#endif
    {   /* who has read how far */
        const VcEvent *last = nullptr;
        for (size_t k = vc_room_event_count(msvc); k > 0 && !last; k--) if (vc_room_event(msvc, k - 1)->sender && !strcmp(vc_room_event(msvc, k - 1)->type, "m.room.message")) last = vc_room_event(msvc, k - 1);
        if (last) {
            QJsonObject rd{{"@zach:example.org", QJsonObject{{"ts", 1}}}, {"@ben:example.org", QJsonObject{{"ts", 1}}}, {"@matt:example.org", QJsonObject{{"ts", 1}}}};
            JsonValue *c = parse({{last->event_id, QJsonObject{{"m.read", rd}}}});
            vc_room_apply_receipts(msvc, c);
            json_free(c);
        }
    }
    m->screen = VC_M_MAIN;
    vc_model_refresh_rooms(m);
    QStringList open;
    if (manyTabs) for (const Chan &c : chans) open << QString("!%1:example.org").arg(c.name);
    open << "!the-lobby:example.org" << QString::fromUtf8(arival->id) << QString::fromUtf8(msvc->id);
    return open;
}

}
