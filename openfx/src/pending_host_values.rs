// SPDX-License-Identifier: GPL-3.0-or-later

use ofx::*;

enum PendingValue {
    String { param: ParamHandle<String>, expected: String, value: String },
    Bool { param: ParamHandle<bool>, expected: bool, value: bool },
    F64 { param: ParamHandle<f64>, expected: f64, value: f64 },
    I32 { param: ParamHandle<i32>, expected: i32, value: i32 },
}

impl PendingValue {
    fn key(&self) -> usize {
        match self {
            Self::String { param, .. } => param.handle() as usize,
            Self::Bool { param, .. } => param.handle() as usize,
            Self::F64 { param, .. } => param.handle() as usize,
            Self::I32 { param, .. } => param.handle() as usize,
        }
    }

    fn commit(self) -> ofx::Result<()> {
        match self {
            Self::String { param, expected, value } => {
                if param.get_value()? == expected { param.set_value(value)?; }
            }
            Self::Bool { param, expected, value } => {
                if param.get_value()? == expected { param.set_value(value)?; }
            }
            Self::F64 { param, expected, value } => {
                if param.get_value()? == expected { param.set_value(value)?; }
            }
            Self::I32 { param, expected, value } => {
                if param.get_value()? == expected { param.set_value(value)?; }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Property {
    Label,
    Hint,
    Enabled,
    Visible,
}

struct PendingProperty {
    key: usize,
    property: Property,
    commit: Box<dyn FnOnce() -> ofx::Result<()>>,
}

#[derive(Default)]
pub(crate) struct PendingHostValues {
    values: Vec<PendingValue>,
    properties: Vec<PendingProperty>,
}

macro_rules! scalar_methods {
    ($peek:ident, $defer:ident, $variant:ident, $ty:ty) => {
        pub(crate) fn $peek(&self, param: &ParamHandle<$ty>) -> Option<$ty> {
            self.values.iter().find_map(|pending| match pending {
                PendingValue::$variant { param: saved, value, .. }
                    if saved.handle() == param.handle() => Some(value.clone()),
                _ => None,
            })
        }

        pub(crate) fn $defer(&mut self, param: &ParamHandle<$ty>, value: $ty) -> ofx::Result<()> {
            let key = param.handle() as usize;
            if let Some(index) = self.values.iter().position(|pending| pending.key() == key) {
                if let PendingValue::$variant { expected, value: queued, .. } = &mut self.values[index] {
                    // The first host value remains the baseline across repeated renders.
                    if value == *expected {
                        self.values.remove(index);
                    } else if value != *queued {
                        *queued = value;
                    }
                    return Ok(());
                }
                return Err(ofx::Error::InvalidValue);
            }
            let expected = param.get_value()?;
            if expected != value {
                self.values.push(PendingValue::$variant { param: param.clone(), expected, value });
            }
            Ok(())
        }
    };
}

impl PendingHostValues {
    scalar_methods!(peek_string, defer_string, String, String);
    scalar_methods!(peek_bool, defer_bool, Bool, bool);
    scalar_methods!(peek_f64, defer_f64, F64, f64);
    scalar_methods!(peek_i32, defer_i32, I32, i32);

    pub(crate) fn forget<T: ParamHandleValue>(&mut self, param: &ParamHandle<T>) {
        let key = param.handle() as usize;
        self.values.retain(|pending| pending.key() != key);
    }

    fn defer_property(&mut self, key: usize, property: Property, commit: Box<dyn FnOnce() -> ofx::Result<()>>) {
        if let Some(pending) = self.properties.iter_mut().find(|pending| pending.key == key && pending.property == property) {
            pending.commit = commit;
        } else {
            self.properties.push(PendingProperty { key, property, commit });
        }
    }

    fn forget_property(&mut self, key: usize, property: Property) {
        self.properties.retain(|pending| pending.key != key || pending.property != property);
    }

    pub(crate) fn defer_label<T: ParamHandleValue + 'static>(&mut self, param: &ParamHandle<T>, value: &str) -> ofx::Result<()> {
        if param.get_label()? == value {
            self.forget_property(param.handle() as usize, Property::Label);
            return Ok(());
        }
        let mut param = param.clone();
        let value = value.to_owned();
        self.defer_property(param.handle() as usize, Property::Label, Box::new(move || {
            if param.get_label()? != value { param.set_label(&value)?; }
            Ok(())
        }));
        Ok(())
    }

    pub(crate) fn defer_hint<T: ParamHandleValue + 'static>(&mut self, param: &ParamHandle<T>, value: &str) -> ofx::Result<()> {
        if param.get_hint()? == value {
            self.forget_property(param.handle() as usize, Property::Hint);
            return Ok(());
        }
        let mut param = param.clone();
        let value = value.to_owned();
        self.defer_property(param.handle() as usize, Property::Hint, Box::new(move || {
            if param.get_hint()? != value { param.set_hint(&value)?; }
            Ok(())
        }));
        Ok(())
    }

    pub(crate) fn defer_enabled<T: ParamHandleValue + 'static>(&mut self, param: &ParamHandle<T>, value: bool) -> ofx::Result<()> {
        if param.get_enabled()? == value {
            self.forget_property(param.handle() as usize, Property::Enabled);
            return Ok(());
        }
        let mut param = param.clone();
        self.defer_property(param.handle() as usize, Property::Enabled, Box::new(move || {
            if param.get_enabled()? != value { param.set_enabled(value)?; }
            Ok(())
        }));
        Ok(())
    }

    pub(crate) fn defer_visible<T: ParamHandleValue + 'static>(&mut self, param: &ParamHandle<T>, value: bool) -> ofx::Result<()> {
        if param.get_secret()? != value {
            self.forget_property(param.handle() as usize, Property::Visible);
            return Ok(());
        }
        let mut param = param.clone();
        self.defer_property(param.handle() as usize, Property::Visible, Box::new(move || {
            if param.get_secret()? == value { param.set_secret(!value)?; }
            Ok(())
        }));
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> ofx::Result<()> {
        // Host setters may synchronously notify the plugin, so detach both queues first.
        let values = std::mem::take(&mut self.values);
        let properties = std::mem::take(&mut self.properties);
        let mut first_error = None;
        for value in values {
            if let Err(error) = value.commit() { first_error.get_or_insert(error); }
        }
        for property in properties {
            if let Err(error) = (property.commit)() { first_error.get_or_insert(error); }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
        self.properties.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty() && self.properties.is_empty()
    }
}
