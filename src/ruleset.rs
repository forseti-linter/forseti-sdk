use crate::core::Diagnostic;
use serde_json::Value;

pub struct RuleContext<'a> {
    pub uri: &'a str,
    pub text: &'a str,
    pub options: &'a Value,
    pub diagnostics: Vec<Diagnostic>,
}
impl<'a> RuleContext<'a> {
    pub fn report(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }
}

pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    fn check(&self, ctx: &mut RuleContext);
}

pub struct Ruleset {
    pub id: String,
    pub rules: Vec<Box<dyn Rule>>,
}
impl Ruleset {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            rules: vec![],
        }
    }
    pub fn with_rule(mut self, rule: Box<dyn Rule>) -> Self {
        self.rules.push(rule);
        self
    }
}

pub fn run_ruleset(
    uri: &str,
    text: &str,
    rs: &Ruleset,
    options: &std::collections::HashMap<String, Value>,
) -> Vec<Diagnostic> {
    let mut all = Vec::new();
    for r in &rs.rules {
        if let Some(opts) = options.get(r.id()) {
            let mut ctx = RuleContext {
                uri,
                text,
                options: opts,
                diagnostics: vec![],
            };
            r.check(&mut ctx);
            all.extend(ctx.diagnostics);
        }
    }
    all
}
