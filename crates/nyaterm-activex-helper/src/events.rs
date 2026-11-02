use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IConnectionPoint, IConnectionPointContainer, IDispatch,
    IDispatch_Impl, IDispatch_Vtbl, ITypeInfo,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, HSTRING, Interface, PCWSTR, implement, interface};

#[interface("336d5562-efa8-482e-8cb3-c5c0fc7a7db6")]
unsafe trait IMsTscAxEvents: IDispatch {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    Connected,
    Disconnected(u32),
    Fatal(u32),
    Reconnecting,
}

#[derive(Clone, Copy)]
enum Kind {
    Connected,
    Disconnected,
    Fatal,
    Reconnecting,
}

#[implement(IMsTscAxEvents)]
struct EventSink {
    ids: HashMap<i32, Kind>,
    pending: Rc<RefCell<VecDeque<ControlEvent>>>,
}

impl IMsTscAxEvents_Impl for EventSink_Impl {}

#[allow(non_snake_case)]
impl IDispatch_Impl for EventSink_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _: u32, _: u32) -> windows::core::Result<ITypeInfo> {
        Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80004001u32 as i32,
        )))
    }
    fn GetIDsOfNames(
        &self,
        _: *const GUID,
        _: *const PCWSTR,
        _: u32,
        _: u32,
        _: *mut i32,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80004001u32 as i32,
        )))
    }
    fn Invoke(
        &self,
        id: i32,
        _: *const GUID,
        _: u32,
        _: DISPATCH_FLAGS,
        params: *const DISPPARAMS,
        _: *mut VARIANT,
        _: *mut EXCEPINFO,
        _: *mut u32,
    ) -> windows::core::Result<()> {
        let Some(kind) = self.ids.get(&id) else {
            return Ok(());
        };
        // The control invokes this sink synchronously on its STA. Never perform
        // IPC, UI callbacks or blocking work inside a COM callback.
        let code = if params.is_null() {
            0
        } else {
            let params = unsafe { &*params };
            if params.cArgs == 0 || params.rgvarg.is_null() {
                0
            } else {
                i32::try_from(unsafe { &*params.rgvarg }).unwrap_or(0) as u32
            }
        };
        let event = match kind {
            Kind::Connected => ControlEvent::Connected,
            Kind::Disconnected => ControlEvent::Disconnected(code),
            Kind::Fatal => ControlEvent::Fatal(code),
            Kind::Reconnecting => ControlEvent::Reconnecting,
        };
        if let Ok(mut queue) = self.pending.try_borrow_mut() {
            if queue.len() < 32 {
                queue.push_back(event);
            } else {
                queue.clear();
                queue.push_back(ControlEvent::Fatal(0));
            }
        }
        Ok(())
    }
}

pub struct EventSubscription {
    point: IConnectionPoint,
    cookie: Option<u32>,
    pending: Rc<RefCell<VecDeque<ControlEvent>>>,
}

impl EventSubscription {
    pub fn new(control: &IDispatch) -> Result<Self, String> {
        let result = (|| -> windows::core::Result<Self> {
            let container: IConnectionPointContainer = control.cast()?;
            let point = unsafe { container.FindConnectionPoint(&IMsTscAxEvents::IID) }?;
            let type_info = unsafe { control.GetTypeInfo(0, 0) }?;
            let mut library = None;
            let mut index = 0;
            unsafe { type_info.GetContainingTypeLib(&mut library, &mut index) }?;
            let library = library.ok_or_else(windows::core::Error::empty)?;
            let event_info = unsafe { library.GetTypeInfoOfGuid(&IMsTscAxEvents::IID) }?;
            let mut ids = HashMap::new();
            for (name, kind) in [
                ("OnConnected", Kind::Connected),
                ("OnLoginComplete", Kind::Connected),
                ("OnDisconnected", Kind::Disconnected),
                ("OnFatalError", Kind::Fatal),
                ("OnAutoReconnecting", Kind::Reconnecting),
                ("OnAutoReconnected", Kind::Connected),
            ] {
                let name = HSTRING::from(name);
                let mut id = 0;
                // Resolve DISPIDs from the installed type library instead of
                // assuming one particular MsTscAx version's event layout.
                unsafe { event_info.GetIDsOfNames(&PCWSTR(name.as_ptr()), 1, &mut id) }?;
                ids.insert(id, kind);
            }
            let pending = Rc::new(RefCell::new(VecDeque::new()));
            let sink: IMsTscAxEvents = EventSink {
                ids,
                pending: Rc::clone(&pending),
            }
            .into();
            let cookie = unsafe { point.Advise(&sink.cast::<windows::core::IUnknown>()?) }?;
            Ok(Self {
                point,
                cookie: Some(cookie),
                pending,
            })
        })();
        result.map_err(|_| "Cannot subscribe to Windows RDP connection events".into())
    }

    pub fn drain(&self) -> Vec<ControlEvent> {
        self.pending.borrow_mut().drain(..).collect()
    }

    pub fn disconnect(&mut self) {
        if let Some(cookie) = self.cookie.take() {
            unsafe {
                let _ = self.point.Unadvise(cookie);
            }
        }
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.disconnect();
    }
}
