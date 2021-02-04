use ffi;
use libc::c_char;
use std::ffi::{CStr, CString};
use std::io;
use std::mem;
use std::rc::Rc;
use std::slice;
use {ErrorType, InterpretResult, Pointer, Type};

fn default_write(_: &mut VM, text: &str) {
    print!("{}", text);
}

fn default_error(_: &mut VM, _type: ErrorType, module: &str, line: i32, message: &str) {
    match _type {
        ErrorType::Compile => println!("[{} line {}] {}", module, line, message),
        ErrorType::Runtime => println!("{}", message),
        ErrorType::StackTrace => println!("[{} line {}] in {}", module, line, message),
    }
}

#[allow(dead_code)]
fn default_load_module(_: &mut VM, name: &str) -> Option<String> {
    use std::fs::File;
    use std::io::Read;
    use std::path::PathBuf;

    let mut buffer = String::new();

    // Look for a file named [name].wren.
    let mut name_path = PathBuf::new();
    name_path.push(name);
    name_path.set_extension("wren");
    let result = File::open(&name_path).map(|mut f| f.read_to_string(&mut buffer));
    if result.is_ok() {
        return Some(buffer);
    }

    // If that fails, treat [name] as a directory and look for module.wren in there.
    name_path.set_extension("");
    name_path.push("module");
    name_path.set_extension("wren");
    buffer.clear();
    let result = File::open(&name_path).map(|mut f| f.read_to_string(&mut buffer));
    if result.is_ok() {
        Some(buffer)
    } else {
        None
    }
}

/// Wrapper around `WrenConfiguration`. Refer to `wren.h` for info on each field.
pub struct Configuration(ffi::WrenConfiguration);

impl Configuration {
    /// Create a new Configuration using `wrenInitConfiguration`.
    ///
    /// This also sets the printing and module loading functions to mimic those used in the CLI interpreter.
    ///
    /// See: https://stackoverflow.com/questions/61318595/writing-to-a-field-in-a-maybeuninit-structure
    ///
    pub fn new() -> Configuration {
        let mut raw: ffi::WrenConfiguration =
            unsafe { mem::MaybeUninit::<ffi::WrenConfiguration>::uninit().assume_init() };
        unsafe { ffi::wrenInitConfiguration(&mut raw) }
        let mut cfg = Configuration(raw);
        cfg.set_write_fn(wren_write_fn!(default_write));
        cfg.set_error_fn(wren_error_fn!(default_error));
        cfg
    }

    pub fn set_reallocate_fn(&mut self, f: ::ReallocateFn) {
        self.0.reallocate_fn = f;
    }

    pub fn set_load_module_fn(&mut self, f: ::LoadModuleFn) {
        self.0.load_module_fn = f;
    }

    pub fn set_bind_foreign_method_fn(&mut self, f: ::BindForeignMethodFn) {
        self.0.bind_foreign_method_fn = f;
    }

    pub fn set_bind_foreign_class_fn(&mut self, f: ::BindForeignClassFn) {
        self.0.bind_foreign_class_fn = f;
    }

    pub fn set_write_fn(&mut self, f: ::WriteFn) {
        self.0.write_fn = f;
    }

    pub fn set_error_fn(&mut self, f: ::ErrorFn) {
        self.0.error_fn = f;
    }

    pub fn set_initial_heap_size(&mut self, size: usize) {
        self.0.initial_heap_size = size;
    }

    pub fn set_min_heap_size(&mut self, size: usize) {
        self.0.min_heap_size = size;
    }

    pub fn set_heap_growth_percent(&mut self, percent: i32) {
        self.0.heap_growth_percent = percent;
    }

    pub fn set_user_data(&mut self, data: Pointer) {
        self.0.user_data = data;
    }
}

/// Reference-counted wrapper around `WrenHandle`.
///
/// Automatically calls `wrenReleaseHandle` when there are no more references.
#[derive(Clone)]
pub struct Handle(Rc<RawHandle>);

struct RawHandle {
    raw: *mut ffi::WrenHandle,
    vm: *mut ffi::WrenVM,
}

impl Drop for RawHandle {
    fn drop(&mut self) {
        unsafe { ffi::wrenReleaseHandle(self.vm, self.raw) }
    }
}

/// Wrapper around `WrenForeignClassMethods`.
#[derive(Copy, Clone)]
pub struct ForeignClassMethods(ffi::WrenForeignClassMethods);

impl ForeignClassMethods {
    pub fn new() -> ForeignClassMethods {
        ForeignClassMethods(ffi::WrenForeignClassMethods {
            allocate: None,
            finalize: None,
        })
    }

    pub fn set_allocate_fn(&mut self, f: ::ForeignMethodFn) {
        self.0.allocate = f;
    }

    pub fn set_finalize_fn(&mut self, f: ::FinalizerFn) {
        self.0.finalize = f;
    }

    #[doc(hidden)]
    pub fn get(&self) -> ffi::WrenForeignClassMethods {
        self.0
    }
}

/// Wrapper around `WrenVM`. Refer to `wren.h` for info on each function.
///
/// Some functions have some additional safety features. In particular:
///
/// 1. Functions that retrieve slot values will perform type checking and return an Option.
///
/// 2. `wrenEnsureSlots` is called automatically where needed.
///
/// 3. Functions that operate on lists will validate their parameters.
pub struct VM {
    raw: *mut ffi::WrenVM,
    owned: bool,
}

impl VM {
    /// Create a new VM.
    pub fn new(cfg: Configuration) -> VM {
        let mut cfg = cfg;
        let raw = unsafe { ffi::wrenNewVM(&mut cfg.0) };
        VM { raw, owned: true }
    }

    /// Create a wrapper around an existing WrenVM pointer.
    ///
    /// This is mainly used by function wrapping macros.
    pub unsafe fn from_ptr(ptr: *mut ffi::WrenVM) -> VM {
        VM {
            raw: ptr,
            owned: false,
        }
    }

    /// Maps to `wrenCollectGarbage`.
    pub fn collect_garbage(&mut self) {
        unsafe { ffi::wrenCollectGarbage(self.raw) }
    }

    /// Maps to `wrenInterpret`.
    pub fn interpret(&mut self, source: &str) -> InterpretResult {
        let source_cstr = CString::new(source).unwrap();
        unsafe { ffi::wrenInterpret(self.raw, source_cstr.as_ptr()) }
    }

    /// Maps to `wrenInterpretInModule`.
    pub fn interpret_in_module(&mut self, module: &str, source: &str) -> InterpretResult {
        let module_cstr = CString::new(module).unwrap();
        let source_cstr = CString::new(source).unwrap();
        unsafe { ffi::wrenInterpretInModule(self.raw, module_cstr.as_ptr(), source_cstr.as_ptr()) }
    }

    /// Convenience function that loads a script from a file and interprets it.
    pub fn interpret_file(&mut self, path: &str) -> Result<InterpretResult, io::Error> {
        use std::fs::File;
        use std::io::Read;

        let mut buffer = String::new();
        let mut file = File::open(path)?;
        file.read_to_string(&mut buffer)?;
        Ok(self.interpret(&buffer))
    }

    /// Maps to `wrenMakeCallHandle`.
    pub fn make_call_handle(&mut self, signature: &str) -> Handle {
        let signature_cstr = CString::new(signature).unwrap();
        let handle = RawHandle {
            raw: unsafe { ffi::wrenMakeCallHandle(self.raw, signature_cstr.as_ptr()) },
            vm: self.raw,
        };
        Handle(Rc::new(handle))
    }

    /// Maps to `wrenCall`.
    pub fn call(&mut self, method: &Handle) -> InterpretResult {
        unsafe { ffi::wrenCall(self.raw, method.0.raw) }
    }

    /*
    /// Maps to `wrenReleaseHandle`.
    pub fn release_handle(&mut self, handle: Handle) {
        unsafe { ffi::wrenReleaseHandle(self.raw, handle.0.raw) }
    }
    */

    /// Maps to `wrenGetSlotCount`.
    pub fn get_slot_count(&mut self) -> i32 {
        unsafe { ffi::wrenGetSlotCount(self.raw) }
    }

    // This gets called automatically where needed.
    fn ensure_slots(&mut self, num_slots: i32) {
        unsafe { ffi::wrenEnsureSlots(self.raw, num_slots) }
    }

    /// Maps to `wrenGetSlotType`.
    pub fn get_slot_type(&mut self, slot: i32) -> Type {
        assert!(
            self.get_slot_count() > slot,
            "Slot {} is out of bounds",
            slot
        );
        unsafe { ffi::wrenGetSlotType(self.raw, slot) }
    }

    /// Maps to `wrenGetSlotBool`.
    ///
    /// Returns `None` if the value in `slot` isn't a bool.
    pub fn get_slot_bool(&mut self, slot: i32) -> Option<bool> {
        if self.get_slot_type(slot) == Type::Bool {
            Some(unsafe { ffi::wrenGetSlotBool(self.raw, slot) != false })
        } else {
            None
        }
    }

    /// Maps to `wrenGetSlotBytes`.
    ///
    /// Returns `None` if the value in `slot` isn't a string.
    pub fn get_slot_bytes(&mut self, slot: i32) -> Option<&[u8]> {
        if self.get_slot_type(slot) == Type::String {
            let mut length: i32 = 0;
            let ptr = unsafe { ffi::wrenGetSlotBytes(self.raw, slot, &mut length) };
            Some(unsafe { slice::from_raw_parts(ptr as *const u8, length as usize) })
        } else {
            None
        }
    }

    /// Maps to `wrenGetSlotDouble`.
    ///
    /// Returns `None` if the value in `slot` isn't a number.
    pub fn get_slot_double(&mut self, slot: i32) -> Option<f64> {
        if self.get_slot_type(slot) == Type::Num {
            Some(unsafe { ffi::wrenGetSlotDouble(self.raw, slot) })
        } else {
            None
        }
    }

    /// Maps to `wrenGetSlotForeign`.
    ///
    /// Returns `None` if the value in `slot` isn't a foreign object.
    pub fn get_slot_foreign(&mut self, slot: i32) -> Option<Pointer> {
        if self.get_slot_type(slot) == Type::Foreign {
            Some(unsafe { ffi::wrenGetSlotForeign(self.raw, slot) })
        } else {
            None
        }
    }

    /// Convenience function that calls `wrenGetSlotForeign` and casts the result.
    ///
    /// This function uses `mem::transmute` internally and is therefore very unsafe.
    pub unsafe fn get_slot_foreign_typed<T>(&mut self, slot: i32) -> &mut T {
        assert!(
            self.get_slot_type(slot) == Type::Foreign,
            "Slot {} must contain a foreign object",
            slot
        );
        mem::transmute::<Pointer, &mut T>(ffi::wrenGetSlotForeign(self.raw, slot))
    }

    /// Maps to `wrenGetSlotString`.
    ///
    /// Returns `None` if the value in `slot` isn't a string.
    pub fn get_slot_string(&mut self, slot: i32) -> Option<&str> {
        if self.get_slot_type(slot) == Type::String {
            let ptr = unsafe { ffi::wrenGetSlotString(self.raw, slot) };
            Some(unsafe { CStr::from_ptr(ptr).to_str().unwrap() })
        } else {
            None
        }
    }

    /// Maps to `wrenGetSlotHandle`.
    pub fn get_slot_handle(&mut self, slot: i32) -> Handle {
        assert!(
            self.get_slot_count() > slot,
            "Slot {} is out of bounds",
            slot
        );
        let handle = RawHandle {
            raw: unsafe { ffi::wrenGetSlotHandle(self.raw, slot) },
            vm: self.raw,
        };
        Handle(Rc::new(handle))
    }

    /// Maps to `wrenSetSlotBool`.
    pub fn set_slot_bool(&mut self, slot: i32, value: bool) {
        self.ensure_slots(slot + 1);
        unsafe { ffi::wrenSetSlotBool(self.raw, slot, value as bool) }
    }

    /// Maps to `wrenSetSlotBytes`.
    pub fn set_slot_bytes(&mut self, slot: i32, bytes: &[u8]) {
        self.ensure_slots(slot + 1);
        let ptr = bytes.as_ptr() as *const c_char;
        let len = bytes.len();
        unsafe { ffi::wrenSetSlotBytes(self.raw, slot, ptr, len) }
    }

    /// Maps to `wrenSetSlotDouble`.
    pub fn set_slot_double(&mut self, slot: i32, value: f64) {
        self.ensure_slots(slot + 1);
        unsafe { ffi::wrenSetSlotDouble(self.raw, slot, value) }
    }

    /// Maps to `wrenSetSlotNewForeign`.
    pub fn set_slot_new_foreign(&mut self, slot: i32, class_slot: i32, size: usize) -> Pointer {
        self.ensure_slots(slot + 1);
        unsafe { ffi::wrenSetSlotNewForeign(self.raw, slot, class_slot, size) }
    }

    /// Convenience function that calls `wrenSetSlotNewForeign` using type information.
    pub fn set_slot_new_foreign_typed<T>(&mut self, slot: i32, class_slot: i32) -> *mut T {
        self.set_slot_new_foreign(slot, class_slot, mem::size_of::<T>()) as *mut T
    }

    /// Maps to `wrenSetSlotNewList`.
    pub fn set_slot_new_list(&mut self, slot: i32) {
        self.ensure_slots(slot + 1);
        unsafe { ffi::wrenSetSlotNewList(self.raw, slot) }
    }

    /// Maps to `wrenSetSlotNull`.
    pub fn set_slot_null(&mut self, slot: i32) {
        self.ensure_slots(slot + 1);
        unsafe { ffi::wrenSetSlotNull(self.raw, slot) }
    }

    /// Maps to `wrenSetSlotString`.
    pub fn set_slot_string(&mut self, slot: i32, s: &str) {
        self.ensure_slots(slot + 1);
        let cstr = CString::new(s).unwrap();
        unsafe { ffi::wrenSetSlotString(self.raw, slot, cstr.as_ptr()) }
    }

    /// Maps to `wrenSetSlotHandle`.
    pub fn set_slot_handle(&mut self, slot: i32, handle: &Handle) {
        self.ensure_slots(slot + 1);
        unsafe { ffi::wrenSetSlotHandle(self.raw, slot, handle.0.raw) }
    }

    /// Maps to `wrenGetListCount`.
    pub fn get_list_count(&mut self, slot: i32) -> i32 {
        if self.get_slot_type(slot) == Type::List {
            unsafe { ffi::wrenGetListCount(self.raw, slot) }
        } else {
            0
        }
    }

    // Checks parameters and converts a negative (relative) list index to an absolute index.
    // Wren already does the latter, but this way we can check if the index is out of bounds.
    // (which Wren doesn't do in release builds)
    fn check_index(&mut self, list_slot: i32, index: i32) -> i32 {
        assert!(
            self.get_slot_type(list_slot) == Type::List,
            "Slot {} must contain a list",
            list_slot
        );
        let list_count = self.get_list_count(list_slot);
        let index = if index < 0 {
            list_count + 1 + index
        } else {
            index
        };
        assert!(index <= list_count, "List index out of bounds");
        index
    }

    /// Maps to `wrenGetListElement`.
    pub fn get_list_element(&mut self, list_slot: i32, index: i32, element_slot: i32) {
        self.ensure_slots(element_slot + 1);
        let index = self.check_index(list_slot, index);
        unsafe { ffi::wrenGetListElement(self.raw, list_slot, index, element_slot) };
    }

    /// Maps to `wrenInsertInList`.
    pub fn insert_in_list(&mut self, list_slot: i32, index: i32, element_slot: i32) {
        assert!(
            element_slot < self.get_slot_count(),
            "No element in slot {}",
            element_slot
        );
        let index = self.check_index(list_slot, index);
        unsafe { ffi::wrenInsertInList(self.raw, list_slot, index, element_slot) };
    }

    /// Maps to `wrenGetVariable`.
    pub fn get_variable(&mut self, module: &str, name: &str, slot: i32) {
        self.ensure_slots(slot + 1);
        let module_cstr = CString::new(module).unwrap();
        let name_cstr = CString::new(name).unwrap();
        unsafe { ffi::wrenGetVariable(self.raw, module_cstr.as_ptr(), name_cstr.as_ptr(), slot) }
    }

    /// Maps to `wrenAbortFiber`.
    pub fn abort_fiber(&mut self, slot: i32) {
        unsafe { ffi::wrenAbortFiber(self.raw, slot) }
    }

    /// Maps to `wrenGetUserData`.
    pub fn get_user_data(&mut self) -> Pointer {
        unsafe { ffi::wrenGetUserData(self.raw) }
    }

    /// Maps to `wrenSetUserData`.
    pub fn set_user_data(&mut self, data: Pointer) {
        unsafe { ffi::wrenSetUserData(self.raw, data) }
    }
}

impl Drop for VM {
    fn drop(&mut self) {
        if self.owned {
            unsafe { ffi::wrenFreeVM(self.raw) }
        }
    }
}
