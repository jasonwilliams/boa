use std::rc::Rc;

use crate::{
    builtins::{Array, IntrinsicObject},
    environments::CompileTimeEnvironment,
    object::{JsObject, PrivateName},
    Context, JsResult, JsString, JsSymbol, JsValue,
};
use boa_gc::{empty_trace, Finalize, Gc, Trace};

mod declarative;
mod private;

use self::declarative::ModuleEnvironment;
pub(crate) use self::{
    declarative::{
        DeclarativeEnvironment, DeclarativeEnvironmentKind, FunctionEnvironment, FunctionSlots,
        LexicalEnvironment, ThisBindingStatus,
    },
    private::PrivateEnvironment,
};

/// The environment stack holds all environments at runtime.
///
/// Environments themselves are garbage collected,
/// because they must be preserved for function calls.
#[derive(Clone, Debug, Trace, Finalize)]
pub(crate) struct EnvironmentStack {
    stack: Vec<Environment>,
    global: Gc<DeclarativeEnvironment>,
    private_stack: Vec<Gc<PrivateEnvironment>>,
}

/// A runtime environment.
#[derive(Clone, Debug, Trace, Finalize)]
pub(crate) enum Environment {
    Declarative(Gc<DeclarativeEnvironment>),
    Object(JsObject),
}

impl Environment {
    /// Returns the declarative environment if it is one.
    pub(crate) const fn as_declarative(&self) -> Option<&Gc<DeclarativeEnvironment>> {
        match self {
            Self::Declarative(env) => Some(env),
            Self::Object(_) => None,
        }
    }
}

impl EnvironmentStack {
    /// Create a new environment stack.
    pub(crate) fn new(global: Gc<DeclarativeEnvironment>) -> Self {
        assert!(matches!(
            global.kind(),
            DeclarativeEnvironmentKind::Global(_)
        ));
        Self {
            stack: Vec::new(),
            global,
            private_stack: Vec::new(),
        }
    }

    /// Replaces the current global with a new global environment.
    pub(crate) fn replace_global(&mut self, global: Gc<DeclarativeEnvironment>) {
        assert!(matches!(
            global.kind(),
            DeclarativeEnvironmentKind::Global(_)
        ));
        self.global = global;
    }

    /// Gets the current global environment.
    pub(crate) fn global(&self) -> &Gc<DeclarativeEnvironment> {
        &self.global
    }

    /// Gets the next outer function environment.
    pub(crate) fn outer_function_environment(&self) -> &Gc<DeclarativeEnvironment> {
        for env in self
            .stack
            .iter()
            .filter_map(Environment::as_declarative)
            .rev()
        {
            if let DeclarativeEnvironmentKind::Function(_) = &env.kind() {
                return env;
            }
        }
        self.global()
    }

    /// Pop all current environments except the global environment.
    pub(crate) fn pop_to_global(&mut self) -> Vec<Environment> {
        let mut envs = Vec::new();
        std::mem::swap(&mut envs, &mut self.stack);
        envs
    }

    /// Get the number of current environments.
    pub(crate) fn len(&self) -> usize {
        self.stack.len()
    }

    /// Truncate current environments to the given number.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.stack.truncate(len);
    }

    /// Extend the current environment stack with the given environments.
    pub(crate) fn extend(&mut self, other: Vec<Environment>) {
        self.stack.extend(other);
    }

    /// `GetThisEnvironment`
    ///
    /// Returns the environment that currently provides a `this` biding.
    ///
    /// More information:
    ///  - [ECMAScript specification][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-getthisenvironment
    pub(crate) fn get_this_environment(&self) -> &DeclarativeEnvironmentKind {
        for env in self.stack.iter().rev() {
            if let Some(decl) = env.as_declarative().filter(|decl| decl.has_this_binding()) {
                return decl.kind();
            }
        }

        self.global().kind()
    }

    /// `GetThisBinding`
    ///
    /// Returns the current `this` binding of the environment.
    /// Note: If the current environment is the global environment, this function returns `Ok(None)`.
    ///
    /// More information:
    ///  - [ECMAScript specification][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-function-environment-records-getthisbinding
    pub(crate) fn get_this_binding(&self) -> JsResult<Option<JsValue>> {
        for env in self.stack.iter().rev() {
            if let Environment::Declarative(decl) = env {
                if let Some(this) = decl.get_this_binding()? {
                    return Ok(Some(this));
                }
            }
        }

        Ok(None)
    }

    /// Push a new object environment on the environments stack.
    pub(crate) fn push_object(&mut self, object: JsObject) {
        self.stack.push(Environment::Object(object));
    }

    /// Push a lexical environment on the environments stack and return it's index.
    pub(crate) fn push_lexical(&mut self, compile_environment: Rc<CompileTimeEnvironment>) -> u32 {
        let num_bindings = compile_environment.num_bindings();

        let (poisoned, with) = {
            // Check if the outer environment is a declarative environment.
            let with = if let Some(env) = self.stack.last() {
                env.as_declarative().is_none()
            } else {
                false
            };

            let environment = self
                .stack
                .iter()
                .rev()
                .find_map(Environment::as_declarative)
                .unwrap_or(self.global());
            (environment.poisoned(), with || environment.with())
        };

        let index = self.stack.len() as u32;

        self.stack.push(Environment::Declarative(Gc::new(
            DeclarativeEnvironment::new(
                DeclarativeEnvironmentKind::Lexical(LexicalEnvironment::new(
                    num_bindings,
                    poisoned,
                    with,
                )),
                compile_environment,
            ),
        )));

        index
    }

    /// Push a function environment on the environments stack.
    pub(crate) fn push_function(
        &mut self,
        compile_environment: Rc<CompileTimeEnvironment>,
        function_slots: FunctionSlots,
    ) {
        let num_bindings = compile_environment.num_bindings();

        let (poisoned, with) = {
            // Check if the outer environment is a declarative environment.
            let with = if let Some(env) = self.stack.last() {
                env.as_declarative().is_none()
            } else {
                false
            };

            let environment = self
                .stack
                .iter()
                .rev()
                .find_map(Environment::as_declarative)
                .unwrap_or(self.global());
            (environment.poisoned(), with || environment.with())
        };

        self.stack.push(Environment::Declarative(Gc::new(
            DeclarativeEnvironment::new(
                DeclarativeEnvironmentKind::Function(FunctionEnvironment::new(
                    num_bindings,
                    poisoned,
                    with,
                    function_slots,
                )),
                compile_environment,
            ),
        )));
    }

    /// Push a module environment on the environments stack.
    pub(crate) fn push_module(&mut self, compile_environment: Rc<CompileTimeEnvironment>) {
        let num_bindings = compile_environment.num_bindings();
        self.stack.push(Environment::Declarative(Gc::new(
            DeclarativeEnvironment::new(
                DeclarativeEnvironmentKind::Module(ModuleEnvironment::new(num_bindings)),
                compile_environment,
            ),
        )));
    }

    /// Pop environment from the environments stack.
    #[track_caller]
    pub(crate) fn pop(&mut self) {
        debug_assert!(!self.stack.is_empty());
        self.stack.pop();
    }

    /// Get the most outer environment.
    pub(crate) fn current_declarative_ref(&self) -> Option<&Gc<DeclarativeEnvironment>> {
        if let Some(env) = self.stack.last() {
            env.as_declarative()
        } else {
            Some(self.global())
        }
    }

    /// Get the compile environment for the current runtime environment.
    pub(crate) fn current_compile_environment(&self) -> Rc<CompileTimeEnvironment> {
        self.stack
            .iter()
            .filter_map(Environment::as_declarative)
            .last()
            .map(|env| env.compile_env())
            .unwrap_or(self.global().compile_env())
    }

    /// Mark that there may be added bindings from the current environment to the next function
    /// environment.
    pub(crate) fn poison_until_last_function(&mut self) {
        for env in self
            .stack
            .iter()
            .rev()
            .filter_map(Environment::as_declarative)
        {
            env.poison();
            if env.compile_env().is_function() {
                return;
            }
        }
        self.global().poison();
    }

    /// Set the value of a lexical binding.
    ///
    /// # Panics
    ///
    /// Panics if the environment or binding index are out of range.
    #[track_caller]
    pub(crate) fn put_lexical_value(
        &mut self,
        environment: BindingLocatorEnvironment,
        binding_index: u32,
        value: JsValue,
    ) {
        let env = match environment {
            BindingLocatorEnvironment::GlobalObject
            | BindingLocatorEnvironment::GlobalDeclarative => self.global(),
            BindingLocatorEnvironment::Stack(index) => self
                .stack
                .get(index as usize)
                .and_then(Environment::as_declarative)
                .expect("must be declarative environment"),
        };
        env.set(binding_index, value);
    }

    /// Set the value of a binding if it is uninitialized.
    ///
    /// # Panics
    ///
    /// Panics if the environment or binding index are out of range.
    #[track_caller]
    pub(crate) fn put_value_if_uninitialized(
        &mut self,
        environment: BindingLocatorEnvironment,
        binding_index: u32,
        value: JsValue,
    ) {
        let env = match environment {
            BindingLocatorEnvironment::GlobalObject
            | BindingLocatorEnvironment::GlobalDeclarative => self.global(),
            BindingLocatorEnvironment::Stack(index) => self
                .stack
                .get(index as usize)
                .and_then(Environment::as_declarative)
                .expect("must be declarative environment"),
        };
        if env.get(binding_index).is_none() {
            env.set(binding_index, value);
        }
    }

    /// Push a private environment to the private environment stack.
    pub(crate) fn push_private(&mut self, environment: Gc<PrivateEnvironment>) {
        self.private_stack.push(environment);
    }

    /// Pop a private environment from the private environment stack.
    pub(crate) fn pop_private(&mut self) {
        self.private_stack.pop();
    }

    /// `ResolvePrivateIdentifier ( privEnv, identifier )`
    ///
    /// More information:
    ///  - [ECMAScript specification][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-resolve-private-identifier
    pub(crate) fn resolve_private_identifier(&self, identifier: JsString) -> Option<PrivateName> {
        // 1. Let names be privEnv.[[Names]].
        // 2. For each Private Name pn of names, do
        //     a. If pn.[[Description]] is identifier, then
        //         i. Return pn.
        // 3. Let outerPrivEnv be privEnv.[[OuterPrivateEnvironment]].
        // 4. Assert: outerPrivEnv is not null.
        // 5. Return ResolvePrivateIdentifier(outerPrivEnv, identifier).
        for environment in self.private_stack.iter().rev() {
            if environment.descriptions().contains(&identifier) {
                return Some(PrivateName::new(identifier, environment.id()));
            }
        }
        None
    }

    /// Return all private name descriptions in all private environments.
    pub(crate) fn private_name_descriptions(&self) -> Vec<&JsString> {
        let mut names = Vec::new();
        for environment in self.private_stack.iter().rev() {
            for name in environment.descriptions() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Indicate if the current environment stack has an object environment.
    pub(crate) fn has_object_environment(&self) -> bool {
        self.stack
            .iter()
            .any(|env| matches!(env, Environment::Object(_)))
    }
}

/// A binding locator contains all information about a binding that is needed to resolve it at runtime.
///
/// Binding locators get created at bytecode compile time and are accessible at runtime via the [`crate::vm::CodeBlock`].
#[derive(Clone, Debug, Eq, Hash, PartialEq, Finalize)]
pub(crate) struct BindingLocator {
    /// Name of the binding.
    name: JsString,

    /// Environment of the binding.
    /// - 0: Global object
    /// - 1: Global declarative environment
    /// - n: Stack environment at index n - 2
    environment: u32,

    /// Index of the binding in the environment.
    binding_index: u32,
}

unsafe impl Trace for BindingLocator {
    empty_trace!();
}

impl BindingLocator {
    /// Creates a new declarative binding locator that has knows indices.
    pub(crate) const fn declarative(
        name: JsString,
        environment_index: u32,
        binding_index: u32,
    ) -> Self {
        Self {
            name,
            environment: environment_index + 1,
            binding_index,
        }
    }

    /// Creates a binding locator that indicates that the binding is on the global object.
    pub(super) const fn global(name: JsString) -> Self {
        Self {
            name,
            environment: 0,
            binding_index: 0,
        }
    }

    /// Returns the name of the binding.
    pub(crate) const fn name(&self) -> &JsString {
        &self.name
    }

    /// Returns if the binding is located on the global object.
    pub(crate) const fn is_global(&self) -> bool {
        self.environment == 0
    }

    /// Returns the environment of the binding.
    pub(crate) fn environment(&self) -> BindingLocatorEnvironment {
        match self.environment {
            0 => BindingLocatorEnvironment::GlobalObject,
            1 => BindingLocatorEnvironment::GlobalDeclarative,
            n => BindingLocatorEnvironment::Stack(n - 2),
        }
    }

    /// Sets the environment of the binding.
    fn set_environment(&mut self, environment: BindingLocatorEnvironment) {
        self.environment = match environment {
            BindingLocatorEnvironment::GlobalObject => 0,
            BindingLocatorEnvironment::GlobalDeclarative => 1,
            BindingLocatorEnvironment::Stack(index) => index + 2,
        };
    }

    /// Returns the binding index of the binding.
    pub(crate) const fn binding_index(&self) -> u32 {
        self.binding_index
    }
}

/// Action that is returned when a fallible binding operation.
#[derive(Debug)]
pub(crate) enum BindingLocatorError {
    /// Trying to mutate immutable binding,
    MutateImmutable,

    /// Indicates that any action is silently ignored.
    Silent,
}

/// The environment in which a binding is located.
#[derive(Clone, Copy, Debug)]
pub(crate) enum BindingLocatorEnvironment {
    GlobalObject,
    GlobalDeclarative,
    Stack(u32),
}

impl Context {
    /// Gets the corresponding runtime binding of the provided `BindingLocator`, modifying
    /// its indexes in place.
    ///
    /// This readjusts a `BindingLocator` to the correct binding if a `with` environment or
    /// `eval` call modified the compile-time bindings.
    ///
    /// Only use if the binding origin is unknown or comes from a `var` declaration. Lexical bindings
    /// are completely removed of runtime checks because the specification guarantees that runtime
    /// semantics cannot add or remove lexical bindings.
    pub(crate) fn find_runtime_binding(&mut self, locator: &mut BindingLocator) -> JsResult<()> {
        if let Some(env) = self.vm.environments.current_declarative_ref() {
            if !env.with() && !env.poisoned() {
                return Ok(());
            }
        }

        let (global, min_index) = match locator.environment() {
            BindingLocatorEnvironment::GlobalObject
            | BindingLocatorEnvironment::GlobalDeclarative => (true, 0),
            BindingLocatorEnvironment::Stack(index) => (false, index),
        };
        let max_index = self.vm.environments.stack.len() as u32;

        for index in (min_index..max_index).rev() {
            match self.environment_expect(index) {
                Environment::Declarative(env) => {
                    if env.poisoned() {
                        let compile = env.compile_env();
                        if compile.is_function() {
                            if let Some(b) = compile.get_binding(locator.name()) {
                                locator.set_environment(b.environment());
                                locator.binding_index = b.binding_index();
                                return Ok(());
                            }
                        }
                    } else if !env.with() {
                        return Ok(());
                    }
                }
                Environment::Object(o) => {
                    let o = o.clone();
                    let key = locator.name().clone();
                    if o.has_property(key.clone(), self)? {
                        if let Some(unscopables) = o.get(JsSymbol::unscopables(), self)?.as_object()
                        {
                            if unscopables.get(key.clone(), self)?.to_boolean() {
                                continue;
                            }
                        }
                        locator.set_environment(BindingLocatorEnvironment::Stack(index));
                        return Ok(());
                    }
                }
            }
        }

        if global {
            let env = self.vm.environments.global();
            if env.poisoned() {
                let compile = env.compile_env();
                if let Some(b) = compile.get_binding(locator.name()) {
                    locator.set_environment(b.environment());
                    locator.binding_index = b.binding_index();
                }
            }
        }

        Ok(())
    }

    /// Finds the object environment that contains the binding and returns the `this` value of the object environment.
    pub(crate) fn this_from_object_environment_binding(
        &mut self,
        locator: &BindingLocator,
    ) -> JsResult<Option<JsObject>> {
        if let Some(env) = self.vm.environments.current_declarative_ref() {
            if !env.with() {
                return Ok(None);
            }
        }

        let min_index = match locator.environment() {
            BindingLocatorEnvironment::GlobalObject
            | BindingLocatorEnvironment::GlobalDeclarative => 0,
            BindingLocatorEnvironment::Stack(index) => index,
        };
        let max_index = self.vm.environments.stack.len() as u32;

        for index in (min_index..max_index).rev() {
            match self.environment_expect(index) {
                Environment::Declarative(env) => {
                    if env.poisoned() {
                        let compile = env.compile_env();
                        if compile.is_function() && compile.get_binding(locator.name()).is_some() {
                            break;
                        }
                    } else if !env.with() {
                        break;
                    }
                }
                Environment::Object(o) => {
                    let o = o.clone();
                    let key = locator.name().clone();
                    if o.has_property(key.clone(), self)? {
                        if let Some(unscopables) = o.get(JsSymbol::unscopables(), self)?.as_object()
                        {
                            if unscopables.get(key.clone(), self)?.to_boolean() {
                                continue;
                            }
                        }
                        return Ok(Some(o));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Checks if the binding pointed by `locator` is initialized.
    ///
    /// # Panics
    ///
    /// Panics if the environment or binding index are out of range.
    pub(crate) fn is_initialized_binding(&mut self, locator: &BindingLocator) -> JsResult<bool> {
        match locator.environment() {
            BindingLocatorEnvironment::GlobalObject => {
                let key = locator.name().clone();
                let obj = self.global_object();
                obj.has_property(key, self)
            }
            BindingLocatorEnvironment::GlobalDeclarative => {
                let env = self.vm.environments.global();
                Ok(env.get(locator.binding_index()).is_some())
            }
            BindingLocatorEnvironment::Stack(index) => match self.environment_expect(index) {
                Environment::Declarative(env) => Ok(env.get(locator.binding_index()).is_some()),
                Environment::Object(obj) => {
                    let key = locator.name().clone();
                    let obj = obj.clone();
                    obj.has_property(key, self)
                }
            },
        }
    }

    /// Get the value of a binding.
    ///
    /// # Panics
    ///
    /// Panics if the environment or binding index are out of range.
    pub(crate) fn get_binding(&mut self, locator: &BindingLocator) -> JsResult<Option<JsValue>> {
        match locator.environment() {
            BindingLocatorEnvironment::GlobalObject => {
                let key = locator.name().clone();
                if key.as_str() == "Array" {
                    Array::init(self.realm());
                }
                let obj = self.global_object();
                obj.try_get(key, self)
            }
            BindingLocatorEnvironment::GlobalDeclarative => {
                let env = self.vm.environments.global();
                Ok(env.get(locator.binding_index()))
            }
            BindingLocatorEnvironment::Stack(index) => match self.environment_expect(index) {
                Environment::Declarative(env) => Ok(env.get(locator.binding_index())),
                Environment::Object(obj) => {
                    let key = locator.name().clone();
                    let obj = obj.clone();
                    obj.get(key, self).map(Some)
                }
            },
        }
    }

    /// Sets the value of a binding.
    ///
    /// # Panics
    ///
    /// Panics if the environment or binding index are out of range.
    #[track_caller]
    pub(crate) fn set_binding(
        &mut self,
        locator: &BindingLocator,
        value: JsValue,
        strict: bool,
    ) -> JsResult<()> {
        match locator.environment() {
            BindingLocatorEnvironment::GlobalObject => {
                let key = locator.name().clone();
                let obj = self.global_object();
                obj.set(key, value, strict, self)?;
            }
            BindingLocatorEnvironment::GlobalDeclarative => {
                let env = self.vm.environments.global();
                env.set(locator.binding_index(), value);
            }
            BindingLocatorEnvironment::Stack(index) => match self.environment_expect(index) {
                Environment::Declarative(decl) => {
                    decl.set(locator.binding_index(), value);
                }
                Environment::Object(obj) => {
                    let key = locator.name().clone();
                    let obj = obj.clone();
                    obj.set(key, value, strict, self)?;
                }
            },
        }
        Ok(())
    }

    /// Deletes a binding if it exists.
    ///
    /// Returns `true` if the binding was deleted.
    ///
    /// # Panics
    ///
    /// Panics if the environment or binding index are out of range.
    pub(crate) fn delete_binding(&mut self, locator: &BindingLocator) -> JsResult<bool> {
        match locator.environment() {
            BindingLocatorEnvironment::GlobalObject => {
                let key = locator.name().clone();
                let obj = self.global_object();
                obj.__delete__(&key.into(), &mut self.into())
            }
            BindingLocatorEnvironment::GlobalDeclarative => Ok(false),
            BindingLocatorEnvironment::Stack(index) => match self.environment_expect(index) {
                Environment::Declarative(_) => Ok(false),
                Environment::Object(obj) => {
                    let key = locator.name().clone();
                    let obj = obj.clone();
                    obj.__delete__(&key.into(), &mut self.into())
                }
            },
        }
    }

    /// Return the environment at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the `index` is out of range.
    pub(crate) fn environment_expect(&self, index: u32) -> &Environment {
        self.vm
            .environments
            .stack
            .get(index as usize)
            .expect("environment index must be in range")
    }
}
