#pragma once
#include <stdint.h>

struct NyaRdpHost;
struct NyaRdpText { const uint16_t* data; uint32_t len; };
struct NyaRdpConfig {
    uint32_t size;
    uint32_t version;
    uintptr_t parent;
    uint64_t generation;
    NyaRdpText server, username, domain, password;
    uint32_t port, width, height, scale, color_depth;
    uint32_t nla, clipboard, admin, reconnect, max_attempts, performance;
};
struct NyaRdpEvent { uint64_t generation; uint32_t kind; int32_t code; uint32_t extended; };
struct NyaRdpRect { int32_t x, y, width, height; uint32_t scale; };
struct NyaRdpStatus {
    uint32_t size;
    uint32_t connected;
    uint32_t drawing_ready;
    uint32_t callbacks;
    uint32_t width, height, color_depth, cipher_strength;
    uint32_t surface_width, surface_height, surface_visible;
};
struct NyaRdpCapabilities { uint32_t size, flags; uint16_t version[128]; };

extern "C" {
int32_t nya_rdp_create(const NyaRdpConfig*, NyaRdpHost**, uint32_t*) noexcept;
int32_t nya_rdp_start(NyaRdpHost*) noexcept;
int32_t nya_rdp_viewport(NyaRdpHost*, const NyaRdpRect*, uint32_t) noexcept;
int32_t nya_rdp_resize(NyaRdpHost*, uint32_t, uint32_t, uint32_t) noexcept;
int32_t nya_rdp_smart_sizing(NyaRdpHost*, uint32_t) noexcept;
int32_t nya_rdp_focus(NyaRdpHost*, uint32_t) noexcept;
int32_t nya_rdp_poll(NyaRdpHost*, NyaRdpEvent*) noexcept;
int32_t nya_rdp_status(NyaRdpHost*, NyaRdpStatus*) noexcept;
int32_t nya_rdp_capabilities(NyaRdpHost*, NyaRdpCapabilities*) noexcept;
int32_t nya_rdp_request_close(NyaRdpHost*, uint32_t*) noexcept;
int32_t nya_rdp_destroy(NyaRdpHost**) noexcept;
}
