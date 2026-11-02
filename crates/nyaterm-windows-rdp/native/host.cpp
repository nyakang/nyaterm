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

HRESULT set_property(IDispatch* object, const wchar_t* name, CComVariant value) {
    if (!object) return E_NOINTERFACE;
    LPOLESTR key = const_cast<LPOLESTR>(name);
    DISPID id = 0;
    HRESULT hr = object->GetIDsOfNames(IID_NULL, &key, 1, LOCALE_INVARIANT, &id);
    if (FAILED(hr)) return hr;
    DISPID named = DISPID_PROPERTYPUT;
    DISPPARAMS args{&value, &named, 1, 1};
    return object->Invoke(id, IID_NULL, LOCALE_INVARIANT, DISPATCH_PROPERTYPUT, &args, nullptr, nullptr, nullptr);
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
    std::array<DISPID, 9> ids_{};
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
            L"OnFocusReleased", L"OnAutoReconnected" };
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
        }
        return S_OK;
    }
};

HRESULT configure(NyaRdpHost& host, const NyaRdpConfig& config) {
    HRESULT hr = host.client->put_Server(text(config.server));
    if (SUCCEEDED(hr)) hr = host.client->put_UserName(text(config.username));
    if (SUCCEEDED(hr)) hr = host.client->put_Domain(text(config.domain));
    if (SUCCEEDED(hr)) hr = host.client->put_DesktopWidth(static_cast<LONG>(config.width));
    if (SUCCEEDED(hr)) hr = host.client->put_DesktopHeight(static_cast<LONG>(config.height));
    if (SUCCEEDED(hr)) hr = host.client->put_ColorDepth(static_cast<LONG>(config.color_depth));
    if (SUCCEEDED(hr)) hr = host.client->get_AdvancedSettings9(&host.advanced);
    if (FAILED(hr)) return hr;
    CComQIPtr<IDispatch> advanced(host.advanced);
    const auto put = [&](const wchar_t* name, CComVariant value) { return set_property(advanced, name, value); };
    if (FAILED(hr = put(L"RDPPort", CComVariant(static_cast<LONG>(config.port))))) return hr;
    if (FAILED(hr = put(L"EnableCredSspSupport", CComVariant(config.nla != 0)))) return hr;
    if (FAILED(hr = put(L"AuthenticationLevel", CComVariant(LONG{1})))) return hr;
    if (FAILED(hr = put(L"ConnectToAdministerServer", CComVariant(config.admin != 0)))) return hr;
    if (FAILED(hr = put(L"RedirectClipboard", CComVariant(config.clipboard != 0)))) return hr;
    if (FAILED(hr = put(L"RedirectDrives", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectPrinters", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectSmartCards", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectPorts", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectDevices", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectPOSDevices", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectDynamicDrives", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"RedirectDynamicDevices", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"AudioCaptureRedirectionMode", CComVariant(false)))) return hr;
    if (FAILED(hr = put(L"EnableAutoReconnect", CComVariant(config.reconnect != 0)))) return hr;
    if (FAILED(hr = put(L"MaxReconnectAttempts", CComVariant(static_cast<LONG>(config.max_attempts))))) return hr;
    if (FAILED(hr = put(L"PerformanceFlags", CComVariant(static_cast<LONG>(config.performance))))) return hr;
    if (FAILED(hr = put(L"SmartSizing", CComVariant(false)))) return hr;
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
    hr = host.client->get_SecuredSettings3(&secured);
    if (FAILED(hr)) return hr;
    if (FAILED(hr = secured->put_KeyboardHookMode(1))) return hr;
    if (FAILED(hr = secured->put_AudioRedirectionMode(2))) return hr;
    if (config.password.len) {
        CComQIPtr<IMsTscNonScriptable> secret(host.control);
        if (!secret) return E_NOINTERFACE;
        CComBSTR password = text(config.password);
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
        config->scale < 100 || config->scale > 500 || config->max_attempts > 200) return E_INVALIDARG;
    const HWND parent = reinterpret_cast<HWND>(config->parent);
    if (!IsWindow(parent) || GetWindowThreadProcessId(parent, nullptr) != GetCurrentThreadId()) return RPC_E_WRONG_THREAD;
    NyaRdpHost* host = new (std::nothrow) NyaRdpHost();
    if (!host) return E_OUTOFMEMORY;
    HRESULT hr = E_FAIL;
    try {
        host->parent = parent; host->generation = config->generation;
        auto create = [&]() -> HRESULT {
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
            *stage = 5;
            result = AtlAxAttachControl(host->control, host->window, &host->container);
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
            *stage = 7;
            return configure(*host, *config);
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
        host->client->get_Connected(&connected); host->client->get_DesktopWidth(&width);
        host->client->get_DesktopHeight(&height); host->client->get_ColorDepth(&depth);
        host->client->get_CipherStrength(&cipher);
    }
    *status = {sizeof(NyaRdpStatus), static_cast<uint32_t>(connected),
        IsWindow(host->window) && GetWindow(host->window, GW_CHILD) ? 1u : 0u,
        host->callbacks, static_cast<uint32_t>(width), static_cast<uint32_t>(height),
        static_cast<uint32_t>(depth), static_cast<uint32_t>(cipher)};
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
