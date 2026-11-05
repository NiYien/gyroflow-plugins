// SPDX-License-Identifier: GPL-3.0-or-later

use gyroflow_plugin_base::{GyroflowPluginBase, ParameterType, Params};
use ofx::*;
use std::collections::HashMap;

fn definition_id(definition: &ParameterType) -> &'static str {
    match definition {
        ParameterType::Group { id, .. } | ParameterType::HiddenString { id }
        | ParameterType::Text { id, .. } | ParameterType::TextBox { id, .. }
        | ParameterType::Slider { id, .. } | ParameterType::Checkbox { id, .. }
        | ParameterType::Select { id, .. } | ParameterType::Button { id, .. } => id,
    }
}

pub(crate) fn arrange_definitions(definitions: Vec<ParameterType>) -> Vec<ParameterType> {
    let (translation, mut result): (Vec<_>, Vec<_>) = definitions.into_iter()
        .partition(|definition| definition_id(definition).starts_with("Translation"));
    if let Some(ParameterType::Group { parameters, .. }) = result.iter_mut().find(|d| definition_id(d) == "AdjustGroup") {
        parameters.extend(translation);
    }
    result
}

pub(crate) fn page_children(definitions: &[ParameterType]) -> Vec<&'static str> {
    let mut result = Vec::new();
    for definition in definitions {
        if let ParameterType::Group { parameters, .. } = definition {
            result.extend(page_children(parameters));
        } else {
            result.push(definition_id(definition));
        }
    }
    result
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    String(String),
    Bool(bool),
    F64(f64, u32),
    I32(i32),
}

enum Parameter {
    String(ParamHandle<String>),
    Bool(ParamHandle<bool>),
    F64(ParamHandle<f64>),
    I32(ParamHandle<i32>),
}

impl Parameter {
    fn read(&self) -> ofx::Result<Value> {
        Ok(match self {
            Self::String(p) => Value::String(p.get_value()?),
            Self::Bool(p) => Value::Bool(p.get_value()?),
            Self::F64(p) => Value::F64(p.get_value()?, p.get_num_keys().unwrap_or_default()),
            Self::I32(p) => Value::I32(p.get_value()?),
        })
    }
}

pub(crate) struct Commit {
    pub changes: Vec<Params>,
    pub commands: Vec<Command>,
    pub binding_changed: bool,
    pub initial: bool,
}

pub(crate) struct Command {
    pub param: Params,
    counter: ParamHandle<i32>,
    baseline: i32,
}

impl Command {
    pub(crate) fn acknowledge(self) -> ofx::Result<()> {
        // The UI restores its pre-click JSON. A missing button key must not leave a latched pulse.
        self.counter.set_value(self.baseline)
    }
}

#[derive(Default)]
pub(crate) struct Protocol {
    parameters: Vec<(Params, Parameter)>,
    buttons: Vec<(Params, ParamHandle<i32>)>,
    before: HashMap<Params, Value>,
    counters: HashMap<Params, i32>,
    user_events: Vec<Params>,
    last_binding: Option<Vec<Option<Value>>>,
    depth: usize,
    pub committing: bool,
    pub time: f64,
}

impl Protocol {
    pub(crate) fn new(set: &ParamSetHandle) -> ofx::Result<Self> {
        let mut result = Self::default();
        result.register(set, GyroflowPluginBase::get_param_definitions())?;
        Ok(result)
    }

    fn register(&mut self, set: &ParamSetHandle, definitions: Vec<ParameterType>) -> ofx::Result<()> {
        for definition in definitions {
            if matches!(&definition, ParameterType::Checkbox { id: "StabilizationSpeedRamp", .. }) { continue; }
            let (name, parameter) = match definition {
                ParameterType::Group { parameters, .. } => { self.register(set, parameters)?; continue; },
                ParameterType::Button { id, .. } => {
                    // Jianying exposes the same int32 counter through paramGetValue as its UI JSON.
                    if let Ok(handle) = set.parameter::<i32>(id) {
                        if let Ok(param) = id.parse::<Params>() { self.buttons.push((param, handle)); }
                    }
                    continue;
                },
                ParameterType::HiddenString { id } | ParameterType::Text { id, .. } | ParameterType::TextBox { id, .. } => (id, Parameter::String(set.parameter(id)?)),
                ParameterType::Checkbox { id, .. } => (id, Parameter::Bool(set.parameter(id)?)),
                ParameterType::Slider { id, .. } => (id, Parameter::F64(set.parameter(id)?)),
                ParameterType::Select { id, .. } => (id, Parameter::I32(set.parameter(id)?)),
            };
            if let Ok(param) = name.parse::<Params>() { self.parameters.push((param, parameter)); }
        }
        Ok(())
    }

    fn snapshot(&self) -> ofx::Result<HashMap<Params, Value>> {
        self.parameters.iter().map(|(name, parameter)| Ok((*name, parameter.read()?))).collect()
    }

    fn binding(values: &HashMap<Params, Value>) -> Vec<Option<Value>> {
        [Params::ProjectPath, Params::ProjectData, Params::EmbeddedLensProfile, Params::EmbeddedPreset, Params::IncludeProjectData, Params::InstanceId]
            .iter().map(|p| values.get(p).cloned()).collect()
    }

    pub(crate) fn begin(&mut self) -> ofx::Result<()> {
        if self.committing { return Ok(()); }
        if self.depth == 0 {
            self.before = self.snapshot()?;
            self.counters = self.buttons.iter().map(|(param, p)| Ok((*param, p.get_value()?))).collect::<ofx::Result<_>>()?;
            self.user_events.clear();
        }
        self.depth += 1;
        Ok(())
    }

    pub(crate) fn changed(&mut self, name: &str, reason: Change, time: f64) {
        if self.committing { return; }
        self.time = time;
        if reason == Change::UserEdited {
            if let Ok(param) = name.parse::<Params>() {
                if !self.user_events.contains(&param) { self.user_events.push(param); }
            }
        }
    }

    pub(crate) fn end(&mut self) -> ofx::Result<Option<Commit>> {
        if self.committing || self.depth == 0 { return Ok(None); }
        self.depth -= 1;
        if self.depth != 0 { return Ok(None); }
        let values = self.snapshot()?;
        let mut changes = Vec::new();
        let mut commands = Vec::new();
        for param in &self.user_events {
            if let Some((_, button)) = self.buttons.iter().find(|(name, _)| name == param) {
                let before = self.counters.get(param).copied().unwrap_or_default();
                let after = button.get_value()?;
                if after as i64 == before as i64 + 1 {
                    commands.push(Command { param: *param, counter: button.clone(), baseline: before });
                }
            } else if self.before.get(param) != values.get(param)
                || matches!(values.get(param), Some(Value::F64(_, keys)) if *keys > 0) {
                // A keyframed curve can change interpolation while its current scalar stays equal.
                changes.push(*param);
            }
        }
        let initial = self.last_binding.is_none();
        let binding_changed = self.last_binding.as_ref() != Some(&Self::binding(&values));
        Ok(Some(Commit { changes, commands, binding_changed, initial }))
    }

    pub(crate) fn finish(&mut self) -> ofx::Result<()> {
        self.last_binding = Some(Self::binding(&self.snapshot()?));
        self.user_events.clear();
        Ok(())
    }

    pub(crate) fn binding_changed(&self) -> ofx::Result<bool> {
        Ok(self.last_binding.as_ref() != Some(&Self::binding(&self.snapshot()?)))
    }

    pub(crate) fn source_replaced_without_new_identity(&self) -> ofx::Result<bool> {
        let Some(previous) = &self.last_binding else { return Ok(false); };
        let current = Self::binding(&self.snapshot()?);
        let was_bound = matches!(&previous[0], Some(Value::String(path)) if !path.is_empty());
        Ok(was_bound && previous[..5] != current[..5] && previous[5] == current[5])
    }

    pub(crate) fn is_display(param: Params) -> bool {
        matches!(param, Params::Status | Params::LoadedProject | Params::LoadedLens | Params::LoadedPreset | Params::TranslationStatus | Params::PluginUpdateNotice)
    }
}
