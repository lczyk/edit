//! Frontend: DSL source code -> IR graph
//!
//! ## Gotchas
//!
//! - Variables are in SSA-ish form: `var x = off; x = x + 1;` creates a new vreg for the
//!   second `x`. But the variable *name* still maps to the new vreg, so later reads see it.
//! - Physical registers (off, etc.) don't follow SSA. Reading them doesn't create a vreg;
//!   the `parse_expression` function handles this by copying to a fresh vreg.
//! - The `raise!` macro captures token_start position at call time. Don't advance before raising.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use stdext::collections::BVec;

use super::*;

macro_rules! raise {
    ($self:ident, $msg:literal) => {{
        let path = $self.path.to_string();
        let (line, column) = $self.position();
        return Err(CompileError { path, line, column, message: $msg.to_string() })
    }};
    ($self:ident, $($arg:tt)*) => {{
        let path = $self.path.to_string();
        let (line, column) = $self.position();
        return Err(CompileError { path, line, column, message: format!($($arg)*) })
    }};
}

struct RegexSpan<'a> {
    pub src: IRCell<'a>,
    pub dst_good: IRCell<'a>,
    pub dst_bad: IRCell<'a>,
    pub capture_groups: BVec<'a, (IRRegCell<'a>, IRRegCell<'a>)>,
    /// The pattern can match without consuming input.
    pub matches_empty: bool,
}

struct Context<'a> {
    loop_start: Option<IRCell<'a>>,
    loop_exit: Option<IRCell<'a>>,
    /// Register holding the input offset at the start of the enclosing loop
    /// iteration, against which the loop's no-progress check compares.
    progress_baseline: Option<IRRegCell<'a>>,
    /// The enclosing loop is an `until` whose guard matches the empty string,
    /// so it exits at end of line before the body can reach an `await input`.
    stranded_await: bool,
    capture_groups: BVec<'a, (IRRegCell<'a>, IRRegCell<'a>)>,
}

pub struct Parser<'a, 'c, 'src> {
    compiler: &'c mut Compiler<'a>,
    path: &'src str,
    src: &'src str,
    pos: usize,
    token_start: usize,
    context: BVec<'a, Context<'a>>,
    variables: HashMap<&'src str, IRRegCell<'a>>,
}

impl<'a, 'c, 'src> Parser<'a, 'c, 'src> {
    pub fn new(compiler: &'c mut Compiler<'a>, path: &'src str, src: &'src str) -> Self {
        let context = BVec::empty();
        Self { compiler, path, src, pos: 0, token_start: 0, context, variables: Default::default() }
    }

    pub fn run(&mut self) -> CompileResult<()> {
        while !self.is_at_eof() {
            let f = self.parse_function()?;
            self.compiler.functions.push(f);
        }
        Ok(())
    }

    fn parse_attributes(&mut self) -> CompileResult<FunctionAttributes<'a>> {
        let mut attributes = FunctionAttributes::default();

        while self.peek() == Some('#') {
            self.pos += 1;
            self.expect('[')?;

            let key = self.read_identifier()?;
            self.expect('=')?;
            let value = arena_clone_str(self.compiler.arena, self.read_string()?);
            self.expect(']')?;

            match key {
                "display_name" => attributes.display_name = Some(value),
                "path" => attributes.paths.push(value),
                "shebang" => attributes.shebangs.push(value),
                "line_comment" => attributes.line_comment = Some(value),
                "block_comment_open" => attributes.block_comment_open = Some(value),
                "block_comment_close" => attributes.block_comment_close = Some(value),
                _ => raise!(self, "unknown attribute '{}'", key),
            }
        }

        if attributes.block_comment_open.is_some() != attributes.block_comment_close.is_some() {
            raise!(self, "block_comment_open and block_comment_close must be specified together");
        }

        Ok(attributes)
    }

    fn parse_function(&mut self) -> CompileResult<Function<'a>> {
        // Reset symbol table for new function
        self.variables = HashMap::from_iter([
            ("off", self.compiler.get_reg(Register::InputOffset)),
            ("ln", self.compiler.get_reg(Register::LINE_NUMBER)),
        ]);

        let attributes = self.parse_attributes()?;

        let public = if self.is_keyword("pub") {
            self.pos += 3;
            true
        } else {
            false
        };

        self.expect_keyword("fn")?;

        let name = arena_clone_str(self.compiler.arena, self.read_identifier()?);

        self.expect('(')?;
        self.expect(')')?;

        let span = self.parse_block()?;

        if let mut last = span.last.borrow_mut()
            && last.wants_next()
        {
            last.set_next(self.compiler.alloc_iri(IRI::Return));
        }

        Ok(Function { name, attributes, body: span.first, public })
    }

    fn parse_block(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect('{')?;

        // TODO: a bit suboptimal to always allocate a noop node
        let mut result: Option<IRSpan> = None;

        while !matches!(self.peek(), Some('}') | None) {
            let s = self.parse_statement()?;
            if let Some(span) = &mut result {
                span.last.borrow_mut().set_next(s.first);
                span.last = s.last;
            } else {
                result = Some(s);
            }
        }

        self.expect('}')?;
        Ok(match result {
            Some(span) => span,
            None => IRSpan::single(self.compiler.alloc_noop()),
        })
    }

    fn parse_statement(&mut self) -> CompileResult<IRSpan<'a>> {
        if self.is_keyword("var") {
            self.parse_var_declaration()
        } else if self.is_keyword("loop") {
            self.parse_loop()
        } else if self.is_keyword("until") {
            self.parse_until()
        } else if self.is_keyword("break") {
            self.parse_break()
        } else if self.is_keyword("continue") {
            self.parse_continue()
        } else if self.is_keyword("return") {
            self.parse_return()
        } else if self.is_keyword("if") {
            self.parse_if()
        } else if self.is_keyword("await") {
            self.parse_await()
        } else if self.is_keyword("yield") {
            self.parse_yield()
        } else if self.is_keyword("save") {
            self.parse_save()
        } else if self.peek().is_some_and(Self::is_ident_start) {
            self.parse_identifier_stmt()
        } else {
            self.mark();
            raise!(self, "unexpected token")
        }
    }

    fn parse_loop(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("loop")?;

        let loop_start = self.compiler.alloc_noop();
        let loop_exit = self.compiler.alloc_noop();
        self.parse_until_impl(loop_start, loop_start, loop_exit, false)
    }

    fn parse_until(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("until")?;

        let re = self.parse_if_regex()?;

        let loop_exit = self.compiler.alloc_noop();
        re.dst_good.borrow_mut().set_next(loop_exit);
        self.parse_until_impl(re.src, re.dst_bad, loop_exit, re.matches_empty)
    }

    fn parse_until_impl(
        &mut self,
        loop_start: IRCell<'a>,
        loop_good: IRCell<'a>,
        loop_exit: IRCell<'a>,
        stranded_await: bool,
    ) -> CompileResult<IRSpan<'a>> {
        // First, save the current input offset.
        // This is used to detect if the loop made any progress.
        let saved_offset = self.compiler.alloc_vreg();
        let first = self.compiler.alloc_iri(IRI::Mov {
            dst: saved_offset,
            src: self.compiler.get_reg(Register::InputOffset),
        });

        self.context.push(
            self.compiler.arena,
            Context {
                loop_start: Some(loop_start),
                loop_exit: Some(loop_exit),
                progress_baseline: Some(saved_offset),
                stranded_await,
                capture_groups: BVec::empty(),
            },
        );
        let block = self.parse_block()?;
        self.context.pop();

        // Force advance the input offset by 1 if it got stuck in the loop iteration.
        //   if input_offset == saved_offset {
        //       input_offset += 1;
        //   }
        let advance = self.compiler.alloc_ir(IR {
            next: Some(first),
            instr: IRI::AddImm { dst: self.compiler.get_reg(Register::InputOffset), imm: 1 },
            offset: usize::MAX,
        });
        // NOTE: It's crucial that we connect the block with the loop before calling collect_interesting_charset,
        // as the until statement's regex is not part of the loop but still counts as an "interesting charset",
        // for the purpose of skipping uninteresting characters.
        first.borrow_mut().set_next(loop_start);
        loop_good.borrow_mut().set_next(block.first);

        // Skip any uninteresting characters before the next loop iteration.
        //   if /.*?/ {}
        let interesting = self.compiler.collect_interesting_charset(loop_start);
        let fast_skip = if interesting.covers_all() {
            advance
        } else {
            let mut skip_charset = interesting.clone();
            skip_charset.invert();
            let skip_charset = self.compiler.intern_charset(&skip_charset);

            self.compiler.alloc_ir(IR {
                next: Some(advance),
                instr: IRI::If {
                    condition: Condition::Charset { cs: skip_charset, min: 1, max: u32::MAX },
                    then: first,
                },
                offset: usize::MAX,
            })
        };

        // Both the skip and the forced advance are stuck-loop recovery: an iteration that
        // did consume input may have stopped on a character a statement after the loop
        // still has to see, so swallowing it here would misattribute it.
        let advance_check = self.compiler.alloc_ir(IR {
            next: Some(first),
            instr: IRI::If {
                condition: Condition::Cmp {
                    lhs: self.compiler.get_reg(Register::InputOffset),
                    rhs: saved_offset,
                    op: ComparisonOp::Eq,
                },
                then: fast_skip,
            },
            offset: usize::MAX,
        });

        if let mut block_last = block.last.borrow_mut()
            && block_last.wants_next()
        {
            block_last.set_next(advance_check);
        }

        Ok(IRSpan { first, last: loop_exit })
    }

    fn parse_break(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("break")?;
        self.expect(';')?;

        if let Some(exit) = self.context.last_mut().and_then(|ctx| ctx.loop_exit) {
            let ir = self.compiler.alloc_noop();
            ir.borrow_mut().set_next(exit);
            Ok(IRSpan::single(ir))
        } else {
            raise!(self, "loop control statement outside of a loop")
        }
    }

    fn parse_continue(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("continue")?;
        self.expect(';')?;

        if let Some(start) = self.context.last_mut().and_then(|ctx| ctx.loop_start) {
            let ir = self.compiler.alloc_noop();
            ir.borrow_mut().set_next(start);
            Ok(IRSpan::single(ir))
        } else {
            raise!(self, "loop control statement outside of a loop")
        }
    }

    fn parse_return(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("return")?;

        // Detector verdicts: `return match;` / `return no_match;` halt the
        // bytecode and surface a bool to [`crate::runtime::Runtime::detect`].
        // Plain `return;` keeps existing semantics (pop call frame or reset
        // VM if the stack is empty).
        if self.is_keyword("match") {
            self.pos += 5;
            self.expect(';')?;
            return Ok(IRSpan::single(self.compiler.alloc_iri(IRI::Halt { result: 1 })));
        }
        if self.is_keyword("no_match") {
            self.pos += 8;
            self.expect(';')?;
            return Ok(IRSpan::single(self.compiler.alloc_iri(IRI::Halt { result: 0 })));
        }

        self.expect(';')?;
        Ok(IRSpan::single(self.compiler.alloc_iri(IRI::Return)))
    }

    fn parse_if(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("if")?;

        if self.peek() == Some('/') {
            self.parse_if_regex_chain()
        } else if self.peek() == Some('$') {
            self.parse_if_saved()
        } else {
            self.parse_if_comparison()
        }
    }

    /// `if $saved { ... }`: the span remembered by `save $N` is a prefix of
    /// the input at the current position. Consumes it on success.
    fn parse_if_saved(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect('$')?;
        let name = self.read_identifier()?;
        if name != "saved" {
            raise!(self, "expected `$saved`");
        }
        self.parse_if_tail(Condition::Saved)
    }

    fn parse_if_regex_chain(&mut self) -> CompileResult<IRSpan<'a>> {
        let mut prev: Option<IRCell<'a>> = None;

        // First, save the current input offset.
        // This is used to restore the position on failed matches.
        let save_reg = self.compiler.alloc_vreg();
        let first = self.compiler.alloc_iri(IRI::Mov {
            dst: save_reg,
            src: self.compiler.get_reg(Register::InputOffset),
        });

        let last = self.compiler.alloc_noop();

        loop {
            let re = self.parse_if_regex()?;

            // Push context with capture groups for the block
            let (loop_start, loop_exit, progress_baseline, stranded_await) = self
                .context
                .last()
                .map(|ctx| {
                    (ctx.loop_start, ctx.loop_exit, ctx.progress_baseline, ctx.stranded_await)
                })
                .unwrap_or((None, None, None, false));
            self.context.push(
                self.compiler.arena,
                Context {
                    loop_start,
                    loop_exit,
                    progress_baseline,
                    stranded_await,
                    capture_groups: re.capture_groups,
                },
            );
            let bl = self.parse_block()?;
            self.context.pop();

            // Connect the previous else branch to form an "else if".
            // If there's no previous one, we're in the first iteration,
            // and so we connect it to the instruction that saves the position.
            prev.unwrap_or(first).borrow_mut().set_next(re.src);

            // Connect the if to the {}.
            re.dst_good.borrow_mut().set_next(bl.first);
            // Connect the end of the {} to the end of the if/else chain.
            if let mut block_last = bl.last.borrow_mut()
                && block_last.wants_next()
            {
                block_last.set_next(last);
            }

            // The "else" branch of the if needs to restore the position.
            let dst_bad = self.compiler.alloc_iri(IRI::Mov {
                dst: self.compiler.get_reg(Register::InputOffset),
                src: save_reg,
            });
            re.dst_bad.borrow_mut().set_next(dst_bad);

            // No else branch? dst_bad (= no hit) means we're done, so make that connection.
            if !self.is_keyword("else") {
                dst_bad.borrow_mut().set_next(last);
                break;
            }

            // Gobble the "else" keyword.
            self.pos += 4;

            // The else branch has a block? That's our dst_bad.
            if !self.is_keyword("if") {
                let bl = self.parse_block()?;
                dst_bad.borrow_mut().set_next(bl.first);
                // Connect the end of the {} to the end of the if/else chain.
                if let mut bl_last = bl.last.borrow_mut()
                    && bl_last.wants_next()
                {
                    bl_last.set_next(last);
                }
                break;
            }

            // Otherwise, we expect an "if" in the next iteration to form an "else if".
            self.expect_keyword("if")?;
            prev = Some(dst_bad);
        }

        Ok(IRSpan { first, last })
    }

    fn parse_if_comparison(&mut self) -> CompileResult<IRSpan<'a>> {
        // Parse: if var1 OP var2 { block } where OP is ==, !=, <, >, <=, >=
        let lhs_name = self.read_identifier()?;
        let lhs_vreg = self.get_variable(lhs_name)?;

        self.mark();
        let op = if self.is_str("==") {
            self.pos += 2;
            ComparisonOp::Eq
        } else if self.is_str("!=") {
            self.pos += 2;
            ComparisonOp::Ne
        } else if self.is_str("<=") {
            self.pos += 2;
            ComparisonOp::Le
        } else if self.is_str(">=") {
            self.pos += 2;
            ComparisonOp::Ge
        } else if self.is_str("<") {
            self.pos += 1;
            ComparisonOp::Lt
        } else if self.is_str(">") {
            self.pos += 1;
            ComparisonOp::Gt
        } else {
            raise!(self, "expected comparison operator (==, !=, <, >, <=, >=)")
        };

        let rhs_name = self.read_identifier()?;
        let rhs_vreg = self.get_variable(rhs_name)?;

        self.parse_if_tail(Condition::Cmp { lhs: lhs_vreg, rhs: rhs_vreg, op })
    }

    /// The block and optional `else` of a non-regex `if`, hung off `condition`.
    fn parse_if_tail(&mut self, condition: Condition<'a>) -> CompileResult<IRSpan<'a>> {
        let dst_good = self.compiler.alloc_noop();
        let dst_bad = self.compiler.alloc_noop();
        let cmp = self.compiler.alloc_ir(IR {
            next: Some(dst_bad),
            instr: IRI::If { condition, then: dst_good },
            offset: usize::MAX,
        });

        let bl = self.parse_block()?;
        dst_good.borrow_mut().set_next(bl.first);

        let end = self.compiler.alloc_noop();

        // Handle optional else branch
        let next = if self.is_keyword("else") {
            self.pos += 4;

            let else_bl = if self.is_keyword("if") { self.parse_if() } else { self.parse_block() };
            let else_bl = else_bl?;

            if let mut else_last = else_bl.last.borrow_mut()
                && else_last.wants_next()
            {
                else_last.set_next(end);
            }

            else_bl.first
        } else {
            end
        };

        dst_bad.borrow_mut().set_next(next);
        if let mut block_last = bl.last.borrow_mut()
            && block_last.wants_next()
        {
            block_last.set_next(end);
        }

        Ok(IRSpan { first: cmp, last: end })
    }

    fn parse_if_regex(&mut self) -> CompileResult<RegexSpan<'a>> {
        let pattern = self.read_regex()?;
        let dst_good = self.compiler.alloc_noop();
        let dst_bad = self.compiler.alloc_noop();
        match regex::parse(self.compiler, pattern, dst_good, dst_bad) {
            Ok((src, capture_groups, matches_empty)) => {
                Ok(RegexSpan { src, dst_good, dst_bad, capture_groups, matches_empty })
            }
            Err(err) => raise!(self, "{}", err),
        }
    }

    fn parse_await(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("await")?;

        let ident = self.read_identifier()?;
        if ident != "input" {
            raise!(self, "expected 'input' after await");
        }

        self.expect(';')?;

        if self.context.last().is_some_and(|ctx| ctx.stranded_await) {
            raise!(
                self,
                "`await input` can never suspend here: the enclosing `until` guard matches the \
                 empty string, so the loop exits at end of line before this is reached. Use \
                 `loop` and gobble up to the delimiter instead."
            );
        }

        let ir = self.compiler.alloc_iri(IRI::AwaitInput);

        // An enclosing loop's no-progress check compares the offset against
        // where the iteration started. Suspending leaves that baseline on the
        // previous line, where it no longer says anything about progress, so
        // retire it -- otherwise the check fires on the next line and forces
        // the offset past its first character. Only the suspending path may
        // do so: with input left on the line `await input` is a no-op, and
        // there the check is all that keeps the loop from spinning.
        let mut baselines: BVec<IRRegCell<'a>> = BVec::empty();
        for ctx in self.context.iter() {
            if let Some(baseline) = ctx.progress_baseline
                && !baselines.iter().any(|&b| std::ptr::eq(b, baseline))
            {
                baselines.push(self.compiler.arena, baseline);
            }
        }
        if baselines.is_empty() {
            return Ok(IRSpan::single(ir));
        }

        let mut last = ir;
        for &baseline in baselines.iter() {
            let node = self.compiler.alloc_iri(IRI::MovImm { dst: baseline, imm: u32::MAX });
            last.borrow_mut().set_next(node);
            last = node;
        }

        let cont = self.compiler.alloc_noop();
        last.borrow_mut().set_next(cont);

        let first = self.compiler.alloc_ir(IR {
            next: Some(cont),
            instr: IRI::If { condition: Condition::EndOfLine, then: ir },
            offset: usize::MAX,
        });

        Ok(IRSpan { first, last: cont })
    }

    fn parse_yield(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("yield")?;

        // Check if this is a capture group reference: yield $n as color
        if self.peek() == Some('$') {
            self.pos += 1;
            let capture_index = self.read_integer()? as usize - 1;

            // Expect "as"
            let kw = self.read_identifier()?;
            if kw != "as" {
                raise!(self, "expected 'as' after capture group reference");
            }

            let color = self.read_identifier()?;
            let kind = self.compiler.intern_highlight_kind(color).value;
            self.expect(';')?;

            let (start_vreg, end_vreg) = self.capture_group(capture_index)?;

            let hs_preg = self.compiler.get_reg(Register::HighlightStart);
            let off_preg = self.compiler.get_reg(Register::InputOffset);
            let off_vreg = self.compiler.alloc_vreg();
            let kind_vreg = self.compiler.alloc_vreg();

            let span = self
                .compiler
                .build_chain()
                // Save offset
                .append(IRI::Mov { dst: off_vreg, src: off_preg })
                // Set start/end temporarily
                .append(IRI::Mov { dst: hs_preg, src: start_vreg })
                .append(IRI::Mov { dst: off_preg, src: end_vreg })
                // Highlight!
                .append(IRI::MovKind { dst: kind_vreg, kind })
                .append(IRI::Flush { kind: kind_vreg })
                // Restore offset
                .append(IRI::Mov { dst: off_preg, src: off_vreg })
                .build();
            Ok(span)
        } else {
            // Normal yield: yield color;
            let color = self.read_identifier()?;
            let kind = self.compiler.intern_highlight_kind(color).value;

            self.expect(';')?;

            let vreg = self.compiler.alloc_vreg();
            let span = self
                .compiler
                .build_chain()
                .append(IRI::MovKind { dst: vreg, kind })
                .append(IRI::Flush { kind: vreg })
                .build();
            Ok(span)
        }
    }

    /// The `(start, end)` registers of capture group `index` (zero-based) in
    /// the innermost regex context.
    fn capture_group(&self, index: usize) -> CompileResult<(IRRegCell<'a>, IRRegCell<'a>)> {
        match self.context.last() {
            Some(ctx) if index < ctx.capture_groups.len() => Ok(ctx.capture_groups[index]),
            Some(_) => raise!(self, "capture group ${} not found in current context", index + 1),
            None => raise!(self, "no regex context available for capture group reference"),
        }
    }

    /// `save $N;`: remember capture group N so a later line can test for it
    /// with `if $saved`.
    fn parse_save(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("save")?;
        if self.peek() != Some('$') {
            raise!(self, "expected a capture group reference after save");
        }
        self.pos += 1;
        let capture_index = self.read_integer()? as usize - 1;
        self.expect(';')?;

        let (start, end) = self.capture_group(capture_index)?;
        Ok(IRSpan::single(self.compiler.alloc_iri(IRI::SaveSpan { start, end })))
    }

    fn parse_var_declaration(&mut self) -> CompileResult<IRSpan<'a>> {
        self.expect_keyword("var")?;

        let name = self.read_identifier()?;
        self.expect('=')?;
        let (expr, vreg) = self.parse_expression()?;
        self.expect(';')?;

        match self.variables.entry(name) {
            Entry::Vacant(e) => _ = e.insert(vreg),
            Entry::Occupied(_) => raise!(self, "variable '{}' already declared", name),
        }

        Ok(expr)
    }

    fn parse_identifier_stmt(&mut self) -> CompileResult<IRSpan<'a>> {
        let name = self.read_identifier()?;

        match self.peek() {
            // foo();
            Some('(') => {
                self.pos += 1;
                self.expect(')')?;
                self.expect(';')?;
                let name = self.compiler.strings.intern(self.compiler.arena, name);
                Ok(IRSpan::single(self.compiler.alloc_iri(IRI::Call { name })))
            }
            // foo = expr;
            Some('=') if !self.is_str("==") => {
                self.pos += 1;

                // A declared variable is written in place, so a value set
                // inside a loop is what the code after the loop reads. Only
                // an undeclared name binds fresh.
                let existing = self.variables.get(name).copied();
                if let Some(dst) = existing
                    && dst.borrow().physical.is_none()
                {
                    self.mark();
                    let ir = match self.peek() {
                        Some('0'..='9') => {
                            let imm = self.read_integer()?;
                            IRI::MovImm { dst, imm }
                        }
                        Some(c) if Self::is_ident_start(c) => {
                            let src_name = self.read_identifier()?;
                            let src = self.get_variable(src_name)?;
                            IRI::Mov { dst, src }
                        }
                        _ => raise!(self, "expected integer or identifier in expression"),
                    };
                    self.expect(';')?;
                    return Ok(IRSpan::single(self.compiler.alloc_iri(ir)));
                }

                let (expr, vreg) = self.parse_expression()?;
                self.expect(';')?;
                self.variables.insert(name, vreg);
                Ok(expr)
            }
            // foo += expr;
            Some('+') if self.is_str("+=") => {
                let lhs_vreg = self.get_variable(name)?;
                self.pos += 2;

                let val = self.read_integer()?;
                self.expect(';')?;

                let ir = self.compiler.alloc_iri(IRI::AddImm { dst: lhs_vreg, imm: val });
                self.variables.insert(name, lhs_vreg);
                Ok(IRSpan::single(ir))
            }
            _ => {
                self.mark();
                raise!(self, "expected '(', '=' or '+=' after identifier")
            }
        }
    }

    fn parse_expression(&mut self) -> CompileResult<(IRSpan<'a>, IRRegCell<'a>)> {
        self.mark();
        let (lhs_span, lhs_vreg) = match self.peek() {
            Some('0'..='9') => {
                let val = self.read_integer()?;
                let vreg = self.compiler.alloc_vreg();
                let ir = self.compiler.alloc_iri(IRI::MovImm { dst: vreg, imm: val });
                (IRSpan::single(ir), vreg)
            }
            Some(c) if Self::is_ident_start(c) => {
                let name = self.read_identifier()?;
                let vreg = self.get_variable(name)?;
                (IRSpan::single(self.compiler.alloc_noop()), vreg)
            }
            _ => raise!(self, "expected integer or identifier in expression"),
        };

        // Check for binary operator
        if self.peek() == Some('+') && !self.is_str("+=") {
            self.pos += 1;

            // Parse right-hand side - only integer literals supported
            let val = self.read_integer()?;
            let add_ir = self.compiler.alloc_iri(IRI::AddImm { dst: lhs_vreg, imm: val });
            lhs_span.last.borrow_mut().set_next(add_ir);
            Ok((IRSpan { first: lhs_span.first, last: add_ir }, lhs_vreg))
        } else if lhs_vreg.borrow().physical.is_some() {
            // For expressions of type `var virtual = physical;`, we need to ensure
            // that we actually copy the physical register into a new virtual one.
            // The remaining code assumes single assignment form, while physical registers are permanent.
            let dst = self.compiler.alloc_vreg();
            let node = self.compiler.alloc_iri(IRI::Mov { dst, src: lhs_vreg });
            Ok((IRSpan::single(node), dst))
        } else {
            Ok((lhs_span, lhs_vreg))
        }
    }

    fn get_variable(&self, name: &str) -> CompileResult<IRRegCell<'a>> {
        match self.variables.get(name) {
            Some(&reg) => Ok(reg),
            None => raise!(self, "undefined variable '{}'", name),
        }
    }

    //
    // vvv Tokenization helpers start here vvv
    //

    fn is_ident_start(ch: char) -> bool {
        ch.is_ascii_alphabetic() || ch == '_'
    }

    fn is_ident_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'
    }

    fn rest(&self) -> &'src str {
        &self.src[self.pos..]
    }

    fn skip_whitespace_comments(&mut self) {
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_ascii_start();
            self.pos = self.src.len() - trimmed.len();

            if let Some(after_comment) = trimmed.strip_prefix("//") {
                self.pos += 2 + after_comment.find('\n').unwrap_or(after_comment.len());
            } else {
                break;
            }
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_whitespace_comments();
        self.rest().chars().next()
    }

    fn is_str(&self, s: &str) -> bool {
        self.rest().starts_with(s)
    }

    fn is_at_eof(&mut self) -> bool {
        self.skip_whitespace_comments();
        self.pos >= self.src.len()
    }

    /// Mark current position for error reporting.
    fn mark(&mut self) {
        self.skip_whitespace_comments();
        self.token_start = self.pos;
    }

    fn position(&self) -> (usize, usize) {
        let before = &self.src[..self.token_start];
        let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
        let column = before.rfind('\n').map_or(before.len(), |i| before.len() - i - 1) + 1;
        (line, column)
    }

    fn expect(&mut self, ch: char) -> CompileResult<()> {
        self.mark();
        if self.rest().starts_with(ch) {
            self.pos += ch.len_utf8();
            Ok(())
        } else {
            raise!(self, "expected '{}'", ch)
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> CompileResult<()> {
        self.mark();
        if let Some(rest) = self.rest().strip_prefix(kw)
            && !rest.chars().next().is_some_and(Self::is_ident_char)
        {
            self.pos += kw.len();
            Ok(())
        } else {
            raise!(self, "expected '{}'", kw)
        }
    }

    /// Check if next token is keyword (without consuming).
    fn is_keyword(&mut self, kw: &str) -> bool {
        self.skip_whitespace_comments();
        self.rest()
            .strip_prefix(kw)
            .is_some_and(|rest| !rest.chars().next().is_some_and(Self::is_ident_char))
    }

    fn read_identifier(&mut self) -> CompileResult<&'src str> {
        self.mark();
        let start = self.pos;

        if !self.rest().chars().next().is_some_and(Self::is_ident_start) {
            raise!(self, "expected identifier");
        }

        let rest = self.rest();
        let len = rest.find(|c| !Self::is_ident_char(c)).unwrap_or(rest.len());
        self.pos += len;
        Ok(&self.src[start..self.pos])
    }

    fn read_integer(&mut self) -> CompileResult<u32> {
        self.mark();
        let start = self.pos;

        let rest = self.rest();
        let len = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        self.pos += len;

        if start == self.pos {
            raise!(self, "expected integer");
        }

        self.src[start..self.pos].parse().map_err(|_| {
            let path = self.path.to_string();
            let (line, column) = self.position();
            CompileError { path, line, column, message: "invalid integer".to_string() }
        })
    }

    fn read_string(&mut self) -> CompileResult<&'src str> {
        self.mark();
        self.expect('"')?;
        let start = self.pos;
        let end = self.find_closing_delimiter(b'"');
        self.pos = end;
        self.expect('"')?;
        Ok(&self.src[start..end])
    }

    fn read_regex(&mut self) -> CompileResult<&'src str> {
        self.mark();
        self.expect('/')?;
        let start = self.pos;
        let end = self.find_closing_delimiter(b'/');
        self.pos = end;
        self.expect('/')?;
        Ok(&self.src[start..end])
    }

    /// Find unescaped closing delimiter (handles `\x` escapes).
    fn find_closing_delimiter(&self, delim: u8) -> usize {
        let bytes = self.rest().as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b if b == delim => return self.pos + i,
                _ => i += 1,
            }
        }
        self.pos + bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use stdext::arena::Arena;

    use crate::compiler::Compiler;

    fn compile(body: &str) -> Result<(), String> {
        let arena = Arena::new(1 << 20).unwrap();
        let mut compiler = Compiler::new(&arena);
        let src = format!(
            "#[display_name = \"T\"]\n\
             #[path = \"**/*.t\"]\n\
             pub fn t() {{\n{body}}}\n"
        );
        compiler.parse("test.lsh", &src).map(|_| ()).map_err(|e| e.message)
    }

    #[test]
    fn an_unpaired_block_comment_attribute_is_rejected() {
        let arena = Arena::new(1 << 20).unwrap();
        let mut compiler = Compiler::new(&arena);
        let src = "#[display_name = \"T\"]\n\
                   #[block_comment_open = \"/*\"]\n\
                   pub fn t() {}\n";
        let err = compiler.parse("test.lsh", src).unwrap_err();
        assert!(err.message.contains("specified together"), "{}", err.message);
    }

    #[test]
    fn an_await_under_an_eol_guard_is_rejected() {
        let err = compile("until /$/ { await input; }\n").unwrap_err();
        assert!(err.contains("can never suspend"), "{err}");
    }

    /// The guard is about a nullable pattern, not about the `$` spelling: a
    /// star-only guard is satisfied by the empty match just as readily.
    #[test]
    fn an_await_under_any_nullable_guard_is_rejected() {
        let err = compile("until /a*/ { await input; }\n").unwrap_err();
        assert!(err.contains("can never suspend"), "{err}");
    }

    #[test]
    fn an_await_under_a_consuming_guard_is_fine() {
        compile("if /\"/ { until /\"/ { await input; } }\n").unwrap();
    }

    #[test]
    fn an_await_in_a_plain_loop_is_fine() {
        compile("loop { await input; }\n").unwrap();
    }

    /// An `if` inside the loop inherits the verdict; the await is no less
    /// stranded for sitting one block deeper.
    #[test]
    fn a_nested_if_does_not_launder_the_await() {
        let err = compile("until /$/ { if /a/ { await input; } }\n").unwrap_err();
        assert!(err.contains("can never suspend"), "{err}");
    }

    #[test]
    fn assigning_an_undeclared_name_still_declares_it() {
        compile("flag = 1;\nif flag == flag { }\n").unwrap();
    }

    #[test]
    fn save_needs_a_capture_in_scope() {
        let err = compile("save $1;\n").unwrap_err();
        assert!(err.contains("no regex context"), "{err}");
        let err = compile("if /a/ { save $1; }\n").unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn if_saved_is_the_only_dollar_condition() {
        let err = compile("if $other { }\n").unwrap_err();
        assert!(err.contains("$saved"), "{err}");
        compile("if /(a)/ { save $1; } if $saved { }\n").unwrap();
    }

    /// An inner `loop` does re-open the door: its own guard is what counts.
    #[test]
    fn an_inner_loop_reopens_the_await() {
        compile("until /$/ { loop { await input; } }\n").unwrap();
    }
}
