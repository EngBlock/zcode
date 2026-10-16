use std::time::Duration;

use anyhow::Result;
use gpui::{App, Context, Task};
use ropey::Rope;

use crate::{EditorMode, InputBaseState};

/// Signature help provider: the signature of the call the caret is in, with
/// the argument being typed singled out.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_signatureHelp
///
/// Typing one of the trigger characters (`(`) opens it; while it's open,
/// every edit and caret move asks again (a retrigger character, `,`, at
/// once; anything else after a pause), and it closes when the provider has
/// nothing for the caret: past the closing parenthesis, or out of the call.
/// Escape and losing focus close it too.
pub trait SignatureHelpProvider {
    /// The characters that open signature help, and the ones that update it
    /// at once while it's open. Asked on each keystroke, so a provider whose
    /// server starts later can answer once it knows.
    fn trigger_characters(&self, cx: &App) -> (Vec<String>, Vec<String>);

    /// textDocument/signatureHelp
    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        context: lsp_types::SignatureHelpContext,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::SignatureHelp>>>;
}

/// The signature help shown, if any.
#[derive(Clone, Debug)]
pub struct SignatureHelpState {
    pub help: lsp_types::SignatureHelp,
    revision: u64,
}

impl SignatureHelpState {
    #[cfg(test)]
    pub(crate) fn for_test(help: lsp_types::SignatureHelp, revision: u64) -> Self {
        Self { help, revision }
    }

    /// Bumped whenever the content changes. See
    /// [`CompletionMenuState::revision`](crate::CompletionMenuState::revision).
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// How long to wait after an edit or caret move before asking again.
const RETRIGGER_DELAY: Duration = Duration::from_millis(120);

impl InputBaseState<EditorMode> {
    /// The signature help shown, if any.
    pub fn signature_help(&self) -> Option<&SignatureHelpState> {
        self.extras.signature_help.as_ref()
    }

    /// Asks for signature help at the caret, as typing a trigger character
    /// does.
    pub fn show_signature_help(&mut self, cx: &mut Context<Self>) {
        self.request_signature_help(None, Duration::ZERO, cx);
    }

    pub fn dismiss_signature_help(&mut self, cx: &mut Context<Self>) {
        self.extras.lsp._signature_help_task = Task::ready(());
        if self.extras.signature_help.take().is_some() {
            cx.notify();
        }
    }

    /// After text was typed: a trigger character opens signature help, and
    /// while it's open a retrigger character updates it at once.
    pub(crate) fn handle_signature_help_trigger(&mut self, new_text: &str, cx: &mut Context<Self>) {
        if self.completion_inserting || self.disabled || self.readonly {
            return;
        }
        let Some(provider) = self.extras.lsp.signature_help_provider.clone() else {
            return;
        };
        let Some(last) = new_text.chars().last() else {
            return;
        };
        let (triggers, retriggers) = provider.trigger_characters(cx);
        let is = |list: &[String]| list.iter().any(|t| t.chars().eq(std::iter::once(last)));
        if is(&triggers) || (self.extras.signature_help.is_some() && is(&retriggers)) {
            self.request_signature_help(Some(last.to_string()), Duration::ZERO, cx);
        }
    }

    /// After an edit or a caret move: while signature help is open, ask
    /// again once things settle, so it follows the argument or closes.
    pub(crate) fn retrigger_signature_help(&mut self, cx: &mut Context<Self>) {
        if self.extras.signature_help.is_some() {
            self.request_signature_help(None, RETRIGGER_DELAY, cx);
        }
    }

    /// Asks the provider about the caret after `delay`, replacing (and so
    /// cancelling) the request before.
    fn request_signature_help(&mut self, trigger: Option<String>, delay: Duration, cx: &mut Context<Self>) {
        let Some(provider) = self.extras.lsp.signature_help_provider.clone() else {
            return;
        };
        let active = self.extras.signature_help.as_ref().map(|state| state.help.clone());
        let context = lsp_types::SignatureHelpContext {
            trigger_kind: if trigger.is_some() {
                lsp_types::SignatureHelpTriggerKind::TRIGGER_CHARACTER
            } else if active.is_some() {
                lsp_types::SignatureHelpTriggerKind::CONTENT_CHANGE
            } else {
                lsp_types::SignatureHelpTriggerKind::INVOKED
            },
            trigger_character: trigger,
            is_retrigger: active.is_some(),
            active_signature_help: active,
        };
        self.extras.lsp._signature_help_task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(task) = this.update(cx, |this, cx| {
                let offset = this.cursor();
                provider.signature_help(&this.text.clone(), offset, context, cx)
            }) else {
                return;
            };
            let help = task.await.ok().flatten().filter(|help| !help.signatures.is_empty());
            this.update(cx, |this, cx| {
                this.extras.signature_help = help.map(|help| {
                    let revision = this.extras.signature_help.as_ref().map_or(0, |state| state.revision + 1);
                    SignatureHelpState { help, revision }
                });
                cx.notify();
            })
            .ok();
        });
    }
}

/// The parts of a signature's label: before the active parameter, the
/// parameter, after it. Parameter labels are substrings of the label or
/// UTF-16 offsets into it.
pub fn split_signature_label(
    signature: &lsp_types::SignatureInformation,
    active_parameter: Option<u32>,
) -> (String, String, String) {
    let label = signature.label.as_str();
    let whole = || (label.to_string(), String::new(), String::new());
    let Some(parameter) = active_parameter
        .or(signature.active_parameter)
        .and_then(|ix| signature.parameters.as_ref()?.get(ix as usize))
    else {
        return whole();
    };
    let range = match &parameter.label {
        lsp_types::ParameterLabel::Simple(name) => label.find(name.as_str()).map(|start| start..start + name.len()),
        lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
            let byte = |utf16: u32| -> Option<usize> {
                let mut units = 0u32;
                for (ix, ch) in label.char_indices() {
                    if units >= utf16 {
                        return Some(ix);
                    }
                    units += ch.len_utf16() as u32;
                }
                (units >= utf16).then_some(label.len())
            };
            byte(*start).zip(byte(*end)).filter(|(s, e)| s <= e).map(|(s, e)| s..e)
        }
    };
    match range {
        Some(range) => (label[..range.start].to_string(), label[range.clone()].to_string(), label[range.end..].to_string()),
        None => whole(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use lsp_types::{ParameterInformation, ParameterLabel, SignatureHelp, SignatureInformation};

    use super::*;
    use crate::test_support::open_editor;

    fn signature(label: &str, parameters: Vec<ParameterLabel>) -> SignatureInformation {
        SignatureInformation {
            label: label.into(),
            documentation: None,
            parameters: Some(parameters.into_iter().map(|label| ParameterInformation { label, documentation: None }).collect()),
            active_parameter: None,
        }
    }

    #[test]
    fn the_active_parameter_is_split_out_of_the_label() {
        // sourcekit-lsp sends UTF-16 offsets.
        let total = signature("total(of: [any Shape]) -> Double", vec![ParameterLabel::LabelOffsets([6, 21])]);
        assert_eq!(
            split_signature_label(&total, Some(0)),
            ("total(".into(), "of: [any Shape]".into(), ") -> Double".into())
        );
        let add = signature("add(int left, int right) -> int", vec![
            ParameterLabel::LabelOffsets([4, 12]),
            ParameterLabel::LabelOffsets([14, 23]),
        ]);
        assert_eq!(split_signature_label(&add, Some(1)).1, "int right");
        // Names, offsets after non-ASCII text, and no active parameter.
        let named = signature("greet(🙂: String)", vec![ParameterLabel::Simple("🙂: String".into())]);
        assert_eq!(split_signature_label(&named, Some(0)).1, "🙂: String");
        let offsets = signature("greet(🙂: String, n: Int)", vec![ParameterLabel::LabelOffsets([18, 24])]);
        assert_eq!(split_signature_label(&offsets, Some(0)).1, "n: Int");
        assert_eq!(split_signature_label(&add, None).0, add.label);
        assert_eq!(split_signature_label(&add, Some(7)).0, add.label);
    }

    /// Answers with a signature while the caret is between `(` and `)`.
    struct Calls {
        asked: Rc<RefCell<Vec<(usize, Option<String>, bool)>>>,
    }

    impl SignatureHelpProvider for Calls {
        fn trigger_characters(&self, _: &App) -> (Vec<String>, Vec<String>) {
            (vec!["(".into()], vec![",".into()])
        }

        fn signature_help(
            &self,
            text: &Rope,
            offset: usize,
            context: lsp_types::SignatureHelpContext,
            _: &mut App,
        ) -> Task<Result<Option<SignatureHelp>>> {
            self.asked.borrow_mut().push((offset, context.trigger_character.clone(), context.is_retrigger));
            let before = text.slice(..offset).to_string();
            let open = before.rfind('(');
            let inside = open.is_some_and(|open| !before[open..].contains(')'));
            let commas = open.map_or(0, |open| before[open..].matches(',').count());
            Task::ready(Ok(inside.then(|| SignatureHelp {
                signatures: vec![signature("f(a: Int, b: Int)", vec![
                    ParameterLabel::Simple("a: Int".into()),
                    ParameterLabel::Simple("b: Int".into()),
                ])],
                active_signature: Some(0),
                active_parameter: Some(commas as u32),
            })))
        }
    }

    #[gpui::test]
    fn it_opens_on_a_trigger_follows_the_arguments_and_closes_after_the_call(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "let x = \n");
        let asked: Rc<RefCell<Vec<(usize, Option<String>, bool)>>> = Default::default();
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.lsp_mut().signature_help_provider = Some(Rc::new(Calls { asked: asked.clone() }));
                state.set_selected_range(8..8, cx);
                state.focus(window, cx);
            })
        });
        let active = |cx: &mut gpui::VisualTestContext| {
            editor.read_with(cx, |state, _| state.signature_help().map(|s| s.help.active_parameter))
        };

        // Typing a name asks nothing; `(` opens it.
        cx.simulate_input("f");
        cx.run_until_parked();
        assert!(asked.borrow().is_empty() && active(&mut cx).is_none());
        cx.simulate_input("(");
        cx.run_until_parked();
        assert_eq!(asked.borrow()[0], (10, Some("(".into()), false));
        assert_eq!(active(&mut cx), Some(Some(0)));

        // `,` updates it at once, to the next parameter.
        cx.simulate_input("1,");
        cx.executor().advance_clock(RETRIGGER_DELAY * 2);
        cx.run_until_parked();
        assert_eq!(active(&mut cx), Some(Some(1)));
        assert!(asked.borrow().iter().any(|(_, trigger, retrigger)| trigger.as_deref() == Some(",") && *retrigger));

        // Past the closing parenthesis the provider has nothing: it closes.
        cx.simulate_input(" 2)");
        cx.executor().advance_clock(RETRIGGER_DELAY * 2);
        cx.run_until_parked();
        assert_eq!(active(&mut cx), None);

        // Escape closes it too.
        cx.simulate_input("; g(");
        cx.run_until_parked();
        assert!(active(&mut cx).is_some());
        cx.dispatch_action(crate::Escape);
        cx.run_until_parked();
        assert_eq!(active(&mut cx), None);
    }
}
