#include "host.h"
#include <windows.h>
#include <commctrl.h>
#include <atlbase.h>
#include <atlhost.h>
#include <deque>
#include <memory>
#include <array>
#include <new>
#pragma warning(disable: 4471)
#include "mstscax.tlh"

namespace {
constexpr uint32_t kAbiVersion = 1;
constexpr size_t kEventLimit = 256;
constexpr HRESULT kBusy = HRESULT_FROM_WIN32(ERROR_BUSY);
class RdpModule : public CAtlModuleT<RdpModule> {};
RdpModule module;

CComBSTR text(NyaRdpText value) {
    return CComBSTR(static_cast<int>(value.len), reinterpret_cast<const wchar_t*>(value.data));
}


}

struct NyaRdpHost {
    DWORD thread = GetCurrentThreadId();
    uint64_t generation = 0;
    HWND parent = nullptr;
    HWND window = nullptr;
    bool ole = false;
    bool closing = false;
    bool overflow = false;
    bool close_requested = false;
    uint32_t close_wait = 0;
    uint32_t callbacks = 0;
    bool desktop_scale = false;
    DWORD cookie = 0;
    CComPtr<IUnknown> container;
    CComPtr<IUnknown> control;
    CComPtr<IMsRdpClient9> client;
    CComPtr<IMsRdpClientAdvancedSettings8> advanced;
    CComPtr<IConnectionPoint> point;
    CComPtr<IDispatch> sink;
    std::deque<NyaRdpEvent> events;

    void emit(uint32_t kind, int32_t code = 0, uint32_t extended = 0) noexcept {
        if (closing && kind != 6) return;
        try {
            if (events.size() >= kEventLimit) { overflow = true; return; }
            events.push_back({generation, kind, code, extended});
        } catch (...) { overflow = true; }
    }
};

namespace {
HRESULT admit(NyaRdpHost* host) {
    if (!host) return E_POINTER;
    return host->thread == GetCurrentThreadId() ? S_OK : RPC_E_WRONG_THREAD;
}

LRESULT CALLBACK child_input(HWND window, UINT message, WPARAM wp, LPARAM lp, UINT_PTR, DWORD_PTR data) {
    auto* host = reinterpret_cast<NyaRdpHost*>(data);
    if ((message == WM_KEYDOWN || message == WM_SYSKEYDOWN) && wp == VK_HOME && GetKeyState(VK_CONTROL) < 0 && GetKeyState(VK_MENU) < 0) {
        SetFocus(host->parent);
        host->emit(7);
        return 0;
    }
    if (message == WM_NCDESTROY) RemoveWindowSubclass(window, child_input, 1);
    return DefSubclassProc(window, message, wp, lp);
}

BOOL CALLBACK subclass_child(HWND window, LPARAM data) {
    SetWindowSubclass(window, child_input, 1, static_cast<DWORD_PTR>(data));
    return TRUE;
}

class Events final : public IDispatch {
    LONG references_ = 1;
    NyaRdpHost* host_;
    std::array<DISPID, 13> ids_{};
public:
    explicit Events(NyaRdpHost* host) : host_(host) {}
    HRESULT initialize() {
        CComPtr<ITypeInfo> info;
        HRESULT hr = host_->client->GetTypeInfo(0, LOCALE_INVARIANT, &info);
        if (FAILED(hr)) return hr;
        CComPtr<ITypeLib> library;
        UINT index = 0;
        hr = info->GetContainingTypeLib(&library, &index);
        if (FAILED(hr)) return hr;
        info.Release();
        hr = library->GetTypeInfoOfGuid(__uuidof(IMsTscAxEvents), &info);
        if (FAILED(hr)) return hr;
        const wchar_t* names[] = { L"OnConnected", L"OnLoginComplete", L"OnDisconnected",
            L"OnFatalError", L"OnConfirmClose", L"OnAutoReconnecting", L"OnAutoReconnecting2",
            L"OnFocusReleased", L"OnAutoReconnected", L"OnConnecting",
            L"OnAuthenticationWarningDisplayed", L"OnAuthenticationWarningDismissed", L"OnLogonError" };
        for (size_t i = 0; i < ids_.size(); ++i) {
            LPOLESTR name = const_cast<LPOLESTR>(names[i]);
            ids_[i] = DISPID_UNKNOWN;
            hr = info->GetIDsOfNames(&name, 1, &ids_[i]);
            // Newer reconnect notifications are optional on older controls.
            if (FAILED(hr) && i != 6 && i != 8) return hr;
        }
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER;
        *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IDispatch && iid != __uuidof(IMsTscAxEvents)) return E_NOINTERFACE;
        *out = static_cast<IDispatch*>(this); AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&references_)); }
    ULONG STDMETHODCALLTYPE Release() override {
        const auto count = InterlockedDecrement(&references_);
        if (!count) delete this;
        return static_cast<ULONG>(count);
    }
    HRESULT STDMETHODCALLTYPE GetTypeInfoCount(UINT* out) override { if (!out) return E_POINTER; *out = 0; return S_OK; }
    HRESULT STDMETHODCALLTYPE GetTypeInfo(UINT, LCID, ITypeInfo**) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE GetIDsOfNames(REFIID, LPOLESTR*, UINT, LCID, DISPID*) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE Invoke(DISPID id, REFIID, LCID, WORD, DISPPARAMS* args, VARIANT*, EXCEPINFO*, UINT*) override {
        if (FAILED(admit(host_))) return RPC_E_WRONG_THREAD;
        struct CallbackScope {
            NyaRdpHost& host;
            explicit CallbackScope(NyaRdpHost& value) : host(value) { ++host.callbacks; }
            ~CallbackScope() { --host.callbacks; }
        } callback(*host_);
        int32_t code = 0;
        bool has_code = false;
        if (args && args->cArgs && args->rgvarg) {
            CComVariant value;
            if (SUCCEEDED(VariantChangeType(&value, &args->rgvarg[args->cArgs - 1], 0, VT_I4))) {
                code = value.lVal; has_code = true;
            }
        }
        size_t kind = ids_.size();
        for (size_t i = 0; i < ids_.size(); ++i) if (ids_[i] != DISPID_UNKNOWN && ids_[i] == id) { kind = i; break; }
        switch (kind) {
        case 0: host_->emit(1); break;
        case 1: host_->emit(2); break;
        case 2: if (has_code) host_->emit(host_->closing ? 6 : 3, code); break;
        case 3: if (has_code) host_->emit(4, code); break;
        case 4:
            // This event asks permission to close; it does not confirm closure.
            if (!args || args->cArgs != 1 || !args->rgvarg ||
                args->rgvarg[0].vt != (VT_BOOL | VT_BYREF) || !args->rgvarg[0].pboolVal) return DISP_E_TYPEMISMATCH;
            *args->rgvarg[0].pboolVal = VARIANT_TRUE;
            break;
        case 5: case 6: host_->emit(5); break;
        case 7: host_->emit(7); break;
        case 8: host_->emit(8); break;
        case 9: host_->emit(9); break;
        case 10: host_->emit(10); break;
        case 11: host_->emit(11); break;
        case 12: if (has_code) host_->emit(12, code); break;
        }
        return S_OK;
    }
};

HRESULT configure(NyaRdpHost& host, const NyaRdpConfig& config, uint32_t* stage) {
    HRESULT hr = S_OK;
    CComPtr<IMsRdpClientNonScriptable3> resources;
    *stage = 100; // Server
    if (FAILED(hr = host.client->put_Server(text(config.server)))) return hr;
    *stage = 101; // UserName
    if (FAILED(hr = host.client->put_UserName(text(config.username)))) return hr;
    *stage = 102; // Domain
    if (FAILED(hr = host.client->put_Domain(text(config.domain)))) return hr;
    *stage = 103; // DesktopWidth
    if (FAILED(hr = host.client->put_DesktopWidth(static_cast<LONG>(config.width)))) return hr;
    *stage = 104; // DesktopHeight
    if (FAILED(hr = host.client->put_DesktopHeight(static_cast<LONG>(config.height)))) return hr;
    *stage = 105; // ColorDepth
    if (FAILED(hr = host.client->put_ColorDepth(static_cast<LONG>(config.color_depth)))) return hr;
    *stage = 106; // AdvancedSettings9
    if (FAILED(hr = host.client->get_AdvancedSettings9(&host.advanced))) return hr;
    *stage = 107; // RDPPort
    if (FAILED(hr = host.advanced->put_RDPPort(static_cast<LONG>(config.port)))) return hr;
    *stage = 108; // EnableCredSspSupport
    if (FAILED(hr = host.advanced->put_EnableCredSspSupport(config.nla ? VARIANT_TRUE : VARIANT_FALSE))) return hr;
    *stage = 109; // AuthenticationLevel
    // Let the system control ask the user before continuing with an untrusted
    // server certificate, as mstsc does. Never silently skip server validation.
    if (FAILED(hr = host.advanced->put_AuthenticationLevel(2))) return hr;
    *stage = 110; // ConnectToAdministerServer
    if (FAILED(hr = host.advanced->put_ConnectToAdministerServer(config.admin ? VARIANT_TRUE : VARIANT_FALSE))) return hr;
    *stage = 111; // RedirectClipboard
    if (FAILED(hr = host.advanced->put_RedirectClipboard(VARIANT_TRUE))) return hr;
    *stage = 112; // RedirectDrives
    if (FAILED(hr = host.advanced->put_RedirectDrives(VARIANT_FALSE))) return hr;
    *stage = 113; // RedirectPrinters
    if (FAILED(hr = host.advanced->put_RedirectPrinters(VARIANT_FALSE))) return hr;
    *stage = 114; // RedirectSmartCards
    if (FAILED(hr = host.advanced->put_RedirectSmartCards(VARIANT_FALSE))) return hr;
    *stage = 115; // RedirectPorts
    if (FAILED(hr = host.advanced->put_RedirectPorts(VARIANT_FALSE))) return hr;
    *stage = 116; // RedirectDevices
    if (FAILED(hr = host.advanced->put_RedirectDevices(VARIANT_FALSE))) return hr;
    *stage = 117; // RedirectPOSDevices
    if (FAILED(hr = host.advanced->put_RedirectPOSDevices(VARIANT_FALSE))) return hr;
    *stage = 118; // IMsRdpClientNonScriptable3
    if (FAILED(hr = host.control->QueryInterface(__uuidof(IMsRdpClientNonScriptable3), reinterpret_cast<void**>(&resources)))) return hr;
    // Authentication dialogs must be modal to the visible application window,
    // not the initially hidden ActiveX child. The typelib uses pointer-sized wireHWND.
    *stage = 135; // UIParentWindowHandle
    if (FAILED(hr = resources->put_UIParentWindowHandle(reinterpret_cast<wireHWND>(host.parent)))) return hr;
    *stage = 119; // RedirectDynamicDrives
    if (FAILED(hr = resources->put_RedirectDynamicDrives(VARIANT_FALSE))) return hr;
    *stage = 120; // RedirectDynamicDevices
    if (FAILED(hr = resources->put_RedirectDynamicDevices(VARIANT_FALSE))) return hr;
    *stage = 121; // AudioCaptureRedirectionMode
    if (FAILED(hr = host.advanced->put_AudioCaptureRedirectionMode(VARIANT_FALSE))) return hr;
    *stage = 122; // EnableAutoReconnect
    if (FAILED(hr = host.advanced->put_EnableAutoReconnect(config.reconnect ? VARIANT_TRUE : VARIANT_FALSE))) return hr;
    *stage = 123; // MaxReconnectAttempts
    if (FAILED(hr = host.advanced->put_MaxReconnectAttempts(static_cast<LONG>(config.max_attempts)))) return hr;
    *stage = 124; // PerformanceFlags
    if (FAILED(hr = host.advanced->put_PerformanceFlags(static_cast<LONG>(config.performance)))) return hr;
    *stage = 125; // SmartSizing
    if (FAILED(hr = host.advanced->put_SmartSizing(VARIANT_FALSE))) return hr;
    CComQIPtr<IMsRdpExtendedSettings> extended(host.control);
    if (extended) {
        CComVariant scale(static_cast<ULONG>(config.scale));
        CComBSTR desktop(L"DesktopScaleFactor"), device(L"DeviceScaleFactor");
        const HRESULT desktop_hr = extended->put_Property(desktop, &scale);
        scale = ULONG{100};
        const HRESULT device_hr = extended->put_Property(device, &scale);
        host.desktop_scale = SUCCEEDED(desktop_hr) && SUCCEEDED(device_hr);
    }
    CComPtr<IMsRdpClientSecuredSettings2> secured;
    *stage = 130;
    hr = host.client->get_SecuredSettings3(&secured);
    if (FAILED(hr)) return hr;
    *stage = 131;
    if (FAILED(hr = secured->put_KeyboardHookMode(1))) return hr;
    *stage = 132;
    if (FAILED(hr = secured->put_AudioRedirectionMode(2))) return hr;
    if (config.password.len) {
        *stage = 133;
        CComQIPtr<IMsTscNonScriptable> secret(host.control);
        if (!secret) return E_NOINTERFACE;
        CComBSTR password = text(config.password);
        *stage = 134;
        hr = secret->put_ClearTextPassword(password);
        if (password.m_str) SecureZeroMemory(password.m_str, SysStringByteLen(password));
    }
    return hr;
}

HRESULT release_host(NyaRdpHost& host) {
    if (host.callbacks) return kBusy;
    if (host.point && host.cookie) {
        const HRESULT hr = host.point->Unadvise(host.cookie);
        if (FAILED(hr)) return hr;
        host.cookie = 0;
    }
    host.sink.Release(); host.point.Release(); host.advanced.Release();
    host.client.Release(); host.control.Release(); host.container.Release();
    if (host.window && !DestroyWindow(host.window) && IsWindow(host.window)) return HRESULT_FROM_WIN32(GetLastError());
    host.window = nullptr;
    if (host.ole) { OleUninitialize(); host.ole = false; }
    return S_OK;
}

}

extern "C" int32_t nya_rdp_create(const NyaRdpConfig* config, NyaRdpHost** out, uint32_t* stage) noexcept try {
    if (!out || !stage) return E_POINTER;
    *out = nullptr;
    *stage = 0;
    if (!config || config->size != sizeof(NyaRdpConfig) || config->version != kAbiVersion) return E_INVALIDARG;
    const auto valid_text = [](NyaRdpText text) {
        if (text.len > 32767 || (text.len && !text.data)) return false;
        for (uint32_t i = 0; i < text.len; ++i) if (text.data[i] == 0) return false;
        return true;
    };
    if (!valid_text(config->server) || !config->server.len || !valid_text(config->username) ||
        !valid_text(config->domain) || !valid_text(config->password) || !config->port || config->port > 65535 ||
        config->width < 200 || config->width > 8192 || config->height < 200 || config->height > 8192 ||
        config->scale < 100 || config->scale > 500 || config->max_attempts > 200 || config->clipboard != 1) return E_INVALIDARG;
    const HWND parent = reinterpret_cast<HWND>(config->parent);
    if (!IsWindow(parent) || GetWindowThreadProcessId(parent, nullptr) != GetCurrentThreadId()) return RPC_E_WRONG_THREAD;
    NyaRdpHost* host = new (std::nothrow) NyaRdpHost();
    if (!host) return E_OUTOFMEMORY;
    HRESULT hr = E_FAIL;
    try {
        host->parent = parent; host->generation = config->generation;
        auto create = [&]() -> HRESULT {
            // DirectComposition applies the parent's child-clipping rules.
            // Keep this window-lifetime style once native children are hosted:
            // other hosts on the same parent can outlive this one.
            *stage = 10;
            const LONG_PTR parent_style = GetWindowLongPtrW(parent, GWL_STYLE);
            if (!(parent_style & WS_CLIPCHILDREN)) {
                SetLastError(ERROR_SUCCESS);
                if (!SetWindowLongPtrW(parent, GWL_STYLE, parent_style | WS_CLIPCHILDREN)) {
                    const DWORD error = GetLastError();
                    if (error != ERROR_SUCCESS) return HRESULT_FROM_WIN32(error);
                }
            }
            *stage = 1;
            HRESULT result = OleInitialize(nullptr);
            if (FAILED(result)) return result;
            host->ole = true;
            *stage = 2;
            if (!AtlAxWinInit()) return E_FAIL;
            host->window = CreateWindowExW(0, L"STATIC", L"NyaTerm RDP", WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0, 0, 1, 1, parent, nullptr, GetModuleHandleW(nullptr), nullptr);
            if (!host->window) return HRESULT_FROM_WIN32(GetLastError());
            *stage = 3;
            result = CoCreateInstance(__uuidof(MsRdpClient9NotSafeForScripting), nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&host->control));
            if (FAILED(result)) return result;
            *stage = 4;
            result = host->control.QueryInterface(&host->client);
            if (FAILED(result)) return result;
            // AttachControl calls ActivateAx with bInited=true, so ATL skips
            // InitNew for caller-created controls. Initialize exactly once,
            // before subscribing and in-place activation, like an OLE container.
            *stage = 8;
            CComQIPtr<IPersistStreamInit> initialization(host->control);
            if (!initialization) return E_NOINTERFACE;
            *stage = 9;
            result = initialization->InitNew();
            if (FAILED(result)) return result;
            *stage = 6;
            CComQIPtr<IConnectionPointContainer> points(host->control);
            if (!points) return E_NOINTERFACE;
            result = points->FindConnectionPoint(__uuidof(IMsTscAxEvents), &host->point);
            if (FAILED(result)) return result;
            auto* sink = new Events(host);
            host->sink.Attach(sink);
            result = sink->initialize();
            if (FAILED(result)) return result;
            result = host->point->Advise(host->sink, &host->cookie);
            if (FAILED(result)) return result;
            *stage = 5;
            result = AtlAxAttachControl(host->control, host->window, &host->container);
            if (FAILED(result)) return result;
            *stage = 7;
            return configure(*host, *config, stage);
        };
        hr = create();
    } catch (...) { hr = E_OUTOFMEMORY; }
    if (SUCCEEDED(hr)) { *out = host; return S_OK; }
    // Even an exception after Advise returns a live owner for deferred cleanup.
    host->closing = true;
    try {
        if (SUCCEEDED(release_host(*host))) delete host;
        else *out = host;
    } catch (...) { *out = host; }
    return hr;
} catch (...) { return E_OUTOFMEMORY; }

extern "C" int32_t nya_rdp_start(NyaRdpHost* host) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (host->closing) return E_UNEXPECTED;
    try { return host->client->Connect(); } catch (...) { return E_FAIL; }
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_viewport(NyaRdpHost* host, const NyaRdpRect* rect, uint32_t visible) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (!rect || rect->width <= 0 || rect->height <= 0 || host->closing) return E_INVALIDARG;
    if (rect->width > 32767 || rect->height > 32767 || rect->scale < 100 || rect->scale > 500) return E_INVALIDARG;
    if (!SetWindowPos(host->window, nullptr, rect->x, rect->y, rect->width, rect->height, SWP_NOACTIVATE | SWP_NOZORDER)) return HRESULT_FROM_WIN32(GetLastError());
    CComQIPtr<IOleInPlaceObject> object(host->control);
    RECT local{0, 0, rect->width, rect->height};
    if (object) { const HRESULT result = object->SetObjectRects(&local, &local); if (FAILED(result)) return result; }
    if (!visible && IsChild(host->window, GetFocus())) SetFocus(host->parent);
    ShowWindow(host->window, visible ? SW_SHOWNOACTIVATE : SW_HIDE);
    EnumChildWindows(host->window, subclass_child, reinterpret_cast<LPARAM>(host));
    return S_OK;
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_resize(NyaRdpHost* host, uint32_t width, uint32_t height, uint32_t scale) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (host->closing) return E_UNEXPECTED;
    if (width < 200 || width > 8192 || height < 200 || height > 8192 || scale < 100 || scale > 500) return E_INVALIDARG;
    return host->client->UpdateSessionDisplaySettings(width, height, 0, 0, 0, scale, 100);
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_smart_sizing(NyaRdpHost* host, uint32_t enabled) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (host->closing || !host->advanced) return E_UNEXPECTED;
    return host->advanced->put_SmartSizing(enabled ? VARIANT_TRUE : VARIANT_FALSE);
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_focus(NyaRdpHost* host, uint32_t child) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (child && host->closing) return E_UNEXPECTED;
    if (!child) {
        if (GetFocus() == host->window || IsChild(host->window, GetFocus())) SetFocus(host->parent);
        return S_OK;
    }
    if (!IsWindowVisible(host->window) || GetForegroundWindow() != GetAncestor(host->parent, GA_ROOT)) return S_FALSE;
    CComQIPtr<IOleInPlaceActiveObject> object(host->control);
    HWND window = host->window;
    if (object) object->GetWindow(&window);
    SetFocus(window); return S_OK;
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_poll(NyaRdpHost* host, NyaRdpEvent* event) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (!event) return E_POINTER;
    if (host->overflow) { host->overflow = false; *event = {host->generation, 4, E_OUTOFMEMORY, 0}; return S_OK; }
    if (host->events.empty()) return S_FALSE;
    *event = host->events.front(); host->events.pop_front();
    if (event->kind == 3 && host->client) {
        ExtendedDisconnectReasonCode reason = exDiscReasonNoInfo;
        if (SUCCEEDED(host->client->get_ExtendedDisconnectReason(&reason))) event->extended = static_cast<uint32_t>(reason);
    }
    return S_OK;
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_status(NyaRdpHost* host, NyaRdpStatus* status) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (!status || status->size != sizeof(NyaRdpStatus)) return E_INVALIDARG;
    short connected = 0; LONG width = 0, height = 0, depth = 0, cipher = 0;
    if (host->client) {
        const HRESULT result = host->client->get_Connected(&connected);
        if (FAILED(result)) return result;
        host->client->get_DesktopWidth(&width);
        host->client->get_DesktopHeight(&height); host->client->get_ColorDepth(&depth);
        host->client->get_CipherStrength(&cipher);
    }
    CComQIPtr<IOleInPlaceObject> object(host->control);
    HWND surface = nullptr;
    RECT surface_rect{};
    const bool ready = object && SUCCEEDED(object->GetWindow(&surface)) &&
        IsWindow(surface) && IsChild(host->window, surface) && GetClientRect(surface, &surface_rect) &&
        surface_rect.right > 0 && surface_rect.bottom > 0;
    *status = {sizeof(NyaRdpStatus), static_cast<uint32_t>(connected), ready ? 1u : 0u,
        host->callbacks, static_cast<uint32_t>(width), static_cast<uint32_t>(height),
        static_cast<uint32_t>(depth), static_cast<uint32_t>(cipher),
        static_cast<uint32_t>(surface_rect.right), static_cast<uint32_t>(surface_rect.bottom),
        ready && IsWindowVisible(surface) ? 1u : 0u};
    return S_OK;
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_capabilities(NyaRdpHost* host, NyaRdpCapabilities* out) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (!out || out->size != sizeof(NyaRdpCapabilities)) return E_INVALIDARG;
    *out = {};
    out->size = sizeof(NyaRdpCapabilities);
    out->flags = 1u | (host->desktop_scale ? 2u : 0u);
    CComBSTR version;
    if (host->client && SUCCEEDED(host->client->get_Version(&version)) && version.m_str) {
        const UINT count = min(version.Length(), 127u);
        for (UINT i = 0; i < count; ++i) out->version[i] = static_cast<uint16_t>(version.m_str[i]);
    }
    return S_OK;
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_request_close(NyaRdpHost* host, uint32_t* wait) noexcept try {
    const HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    if (!wait) return E_POINTER;
    if (host->close_requested) { *wait = host->close_wait; return S_OK; }
    host->closing = true;
    if (IsChild(host->window, GetFocus())) SetFocus(host->parent);
    ShowWindow(host->window, SW_HIDE);
    if (!host->client) { host->close_requested = true; *wait = 0; return S_OK; }
    ControlCloseStatus status = controlCloseCanProceed;
    const HRESULT result = host->client->RequestClose(&status);
    *wait = status == controlCloseWaitForEvents ? 1u : 0u;
    if (SUCCEEDED(result)) { host->close_requested = true; host->close_wait = *wait; }
    return result;
} catch (...) { return E_FAIL; }

extern "C" int32_t nya_rdp_destroy(NyaRdpHost** owner) noexcept try {
    if (!owner || !*owner) return S_OK;
    const HRESULT hr = admit(*owner); if (FAILED(hr)) return hr;
    (*owner)->closing = true;
    const HRESULT result = release_host(**owner);
    if (FAILED(result)) return result;
    delete *owner; *owner = nullptr; return S_OK;
} catch (...) { return E_FAIL; }

// Read the actual system control in the offline Windows initialization regression test.
extern "C" int32_t nya_rdp_verify_connection_policy(NyaRdpHost* host) noexcept try {
    HRESULT hr = admit(host); if (FAILED(hr)) return hr;
    VARIANT_BOOL enabled = VARIANT_TRUE;
    unsigned int authentication = 0;
    if (FAILED(hr = host->advanced->get_AuthenticationLevel(&authentication))) return hr;
    if (authentication != 2) return E_FAIL;
    if (FAILED(hr = host->advanced->get_RedirectClipboard(&enabled))) return hr;
    if (enabled != VARIANT_TRUE) return E_FAIL;
    CComQIPtr<IMsRdpClientNonScriptable3> resources(host->control);
    if (!resources) return E_NOINTERFACE;
    wireHWND ui_parent = nullptr;
    if (FAILED(hr = resources->get_UIParentWindowHandle(&ui_parent))) return hr;
    if (reinterpret_cast<HWND>(ui_parent) != host->parent) return E_FAIL;
    if (FAILED(hr = resources->get_RedirectDynamicDrives(&enabled))) return hr;
    if (enabled != VARIANT_FALSE) return E_FAIL;
    if (FAILED(hr = resources->get_RedirectDynamicDevices(&enabled))) return hr;
    return enabled == VARIANT_FALSE ? S_OK : E_FAIL;
} catch (...) { return E_FAIL; }
