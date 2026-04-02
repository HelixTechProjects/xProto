use super::{
    lexer_impl::{Lexer, LexerError},
    loc::Loc,
    str_lit::{StrLit, StrLitDecodeError},
    token::{Token, TokenWithLocation},
};

#[derive(Debug, thiserror::Error)]
pub enum TokenizerError {
    #[error(transparent)]
    LexerError(#[from] LexerError),

    #[error(transparent)]
    StrLitDecodeError(#[from] StrLitDecodeError),

    #[error("分词器出错：{} 行 {} 列 ",  .0.line, .0.col)]
    InternalError(Loc),

    #[error("解析出错：{} 行 {} 列 错误",  .0.line, .0.col)]
    IncorrectInput(Loc),

    #[error("Not allowed in this context: {0}")]
    NotAllowedInThisContext(&'static str),

    #[error("Unexpected end of input")]
    UnexpectedEof,

    #[error("解析出错：{} 行 {} 列 字符应该为 string",  .0.line, .0.col)]
    ExpectStrLit(Loc),

    #[error("解析出错：{} 行 {} 列 字符应该为 int",  .0.line, .0.col)]
    ExpectIntLit(Loc),

    #[error("解析出错：{} 行 {} 列 字符应该为 float",  .0.line, .0.col)]
    ExpectFloatLit(Loc),

    #[error("解析出错：{} 行 {} 列 字符应该为变量名",  .0.line, .0.col)]
    ExpectIdent(Loc),

    #[error("解析出错：{} 行 {} 列 字符应该为 {}",  .1.line, .1.col, 0)]
    ExpectNamedIdent(String, Loc),

    #[error("解析 {} 出错：{} 行 {} 列 字符应该为 `{}`", .1, .2.line, .2.col, .0)]
    ExpectChar(char, &'static str, Loc),

    #[error("Expecting any char of: {}", .0.iter().map(|c| format!("`{}`", c)).collect::<Vec<_>>().join(", "))]
    ExpectAnyChar(Vec<char>),
}

impl TokenizerError {
    pub fn loc(&self) -> Loc {
        match self {
            TokenizerError::InternalError(loc)
            | TokenizerError::IncorrectInput(loc)
            | TokenizerError::ExpectStrLit(loc)
            | TokenizerError::ExpectIntLit(loc)
            | TokenizerError::ExpectFloatLit(loc)
            | TokenizerError::ExpectIdent(loc) => *loc,
            TokenizerError::ExpectNamedIdent(_, loc) => *loc,
            TokenizerError::ExpectChar(_, _, loc) => *loc,

            _ => Loc { line: 0, col: 0 },
        }
    }
}

pub type TokenizerResult<R> = Result<R, TokenizerError>;

#[derive(Clone)]
pub struct Tokenizer<'a> {
    lexer: Lexer<'a>,
    next_token: Option<TokenWithLocation>,
    last_token_loc: Option<Loc>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a str) -> Tokenizer<'a> {
        Tokenizer {
            lexer: Lexer::new(input),
            next_token: None,
            last_token_loc: None,
        }
    }
    //

    pub fn loc(&self) -> Loc {
        // After lookahead return the location of the next token
        self.next_token
            .as_ref()
            .map(|t| t.loc.clone())
            // After token consumed return the location of that token
            .or(self.last_token_loc.clone())
            // Otherwise return the position of lexer
            .unwrap_or(self.lexer.loc)
    }

    pub fn lookahead_loc(&mut self) -> Loc {
        drop(self.lookahead());
        // TODO: does not handle EOF properly
        self.loc()
    }

    fn lookahead(&mut self) -> TokenizerResult<Option<&Token>> {
        Ok(match self.next_token {
            Some(ref token) => Some(&token.token),
            None => {
                self.next_token = self.lexer.next_token()?;
                self.last_token_loc = self.next_token.as_ref().map(|t| t.loc.clone());
                match self.next_token {
                    Some(ref token) => Some(&token.token),
                    None => None,
                }
            }
        })
    }

    pub fn lookahead_some(&mut self) -> TokenizerResult<&Token> {
        match self.lookahead()? {
            Some(token) => Ok(token),
            None => Err(TokenizerError::UnexpectedEof),
        }
    }
    //
    fn next(&mut self) -> TokenizerResult<Option<Token>> {
        self.lookahead()?;
        Ok(self
            .next_token
            .take()
            .map(|TokenWithLocation { token, .. }| token))
    }

    pub fn next_some(&mut self) -> TokenizerResult<Token> {
        match self.next()? {
            Some(token) => Ok(token),
            None => Err(TokenizerError::UnexpectedEof),
        }
    }

    /// Can be called only after lookahead, otherwise it's error
    pub fn advance(&mut self) -> TokenizerResult<Token> {
        self.next_token
            .take()
            .map(|TokenWithLocation { token, .. }| token)
            .ok_or(TokenizerError::InternalError(self.loc()))
    }

    /// No more tokens
    pub fn syntax_eof(&mut self) -> TokenizerResult<bool> {
        Ok(self.lookahead()?.is_none())
    }

    pub fn next_token_if_map<P, R>(&mut self, p: P) -> TokenizerResult<Option<R>>
    where
        P: FnOnce(&Token) -> Option<R>,
    {
        self.lookahead()?;
        let v = match self.next_token {
            Some(ref token) => match p(&token.token) {
                Some(v) => v,
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        self.next_token = None;
        Ok(Some(v))
    }

    pub fn next_token_check_map<P, R, E>(&mut self, p: P) -> Result<R, E>
    where
        P: FnOnce(&Token) -> Result<R, E>,
        E: From<TokenizerError>,
    {
        self.lookahead()?;
        let r = match self.next_token {
            Some(ref token) => p(&token.token)?,
            None => return Err(TokenizerError::UnexpectedEof.into()),
        };
        self.next_token = None;
        Ok(r)
    }

    fn next_token_if<P>(&mut self, p: P) -> TokenizerResult<Option<Token>>
    where
        P: FnOnce(&Token) -> bool,
    {
        self.next_token_if_map(|token| if p(token) { Some(token.clone()) } else { None })
    }

    pub fn next_ident_if_in(&mut self, idents: &[&str]) -> TokenizerResult<Option<String>> {
        let v = match self.lookahead()? {
            Some(&Token::Ident(ref next)) => {
                if idents.into_iter().find(|&i| i == next).is_some() {
                    next.clone()
                } else {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        };
        self.advance()?;
        Ok(Some(v))
    }

    pub fn next_ident_if_eq(&mut self, word: &str) -> TokenizerResult<bool> {
        Ok(self.next_ident_if_in(&[word])? != None)
    }

    pub fn next_token_is_annotation(&mut self) -> TokenizerResult<(bool, String)> {
        if let Some(token) = self.lookahead()? {
            match token {
                Token::Annotation(_) => {
                    let token = self.advance()?;
                    match token {
                        Token::Annotation(raw) => Ok((true, raw)),
                        _ => Err(TokenizerError::InternalError(self.loc())),
                    }
                }
                _ => Ok((false, String::default())),
            }
        } else {
            Ok((false, String::default()))
        }
    }

    pub fn next_ident_expect_eq(&mut self, word: &str) -> TokenizerResult<()> {
        if self.next_ident_if_eq(word)? {
            Ok(())
        } else {
            Err(TokenizerError::ExpectNamedIdent(
                word.to_owned(),
                self.lookahead_loc(),
            ))
        }
    }

    pub fn next_ident_if_eq_error(&mut self, word: &'static str) -> TokenizerResult<()> {
        if self.clone().next_ident_if_eq(word)? {
            // TODO: which context?
            return Err(TokenizerError::NotAllowedInThisContext(word));
        }
        Ok(())
    }

    pub fn next_symbol_if_eq(&mut self, symbol: char) -> TokenizerResult<bool> {
        Ok(self.next_token_if(|token| match token {
            &Token::Symbol(c) if c == symbol => true,
            _ => false,
        })? != None)
    }

    pub fn next_symbol_expect_eq(
        &mut self,
        symbol: char,
        desc: &'static str,
    ) -> TokenizerResult<()> {
        if self.lookahead_is_symbol(symbol)? {
            self.advance()?;
            Ok(())
        } else {
            Err(TokenizerError::ExpectChar(symbol, desc, self.loc()))
        }
    }

    pub fn next_symbol_expect_eq_oneof(&mut self, symbols: &[char]) -> TokenizerResult<char> {
        for symbol in symbols {
            if let Ok(()) = self.next_symbol_expect_eq(*symbol, "ignored") {
                return Ok(*symbol);
            }
        }
        Err(TokenizerError::ExpectAnyChar(symbols.to_owned()))
    }

    pub fn skip_symbol_until_eq(&mut self, symbol: char) {
        loop {
            let ret = self.lookahead_is_symbol(symbol);
            if ret.is_err() {
                break;
            }
            let ret2 = self.advance();
            if ret2.is_err() {
                break;
            }

            if let Ok(is_find) = ret {
                if is_find {
                    break;
                }
            }
        }
    }

    pub fn skip_pair_symbol(&mut self, symbol: char) -> TokenizerResult<()> {
        let start_symbol = if symbol == '}' {
            '{'
        } else if symbol == ')' {
            '('
        } else {
            return Err(TokenizerError::ExpectChar(
                symbol,
                "不是成对符号",
                self.loc(),
            ));
        };

        let mut skip_num = 1;

        loop {
            match self.lookahead()? {
                Some(&Token::Symbol(c)) => {
                    if c == start_symbol {
                        skip_num += 1;
                    } else if c == symbol {
                        skip_num -= 1;
                    }
                }
                _ => {}
            }
            let _ = self.advance()?;
            if skip_num <= 0 {
                break;
            }
        }
        Ok(())
    }

    pub fn lookahead_is_str_lit(&mut self) -> TokenizerResult<bool> {
        Ok(match self.lookahead()? {
            Some(&Token::StrLit(..)) => true,
            _ => false,
        })
    }

    pub fn lookahead_is_int_lit(&mut self) -> TokenizerResult<bool> {
        Ok(match self.lookahead()? {
            Some(&Token::IntLit(..)) => true,
            _ => false,
        })
    }

    pub fn lookahead_if_symbol(&mut self) -> TokenizerResult<Option<char>> {
        Ok(match self.lookahead()? {
            Some(&Token::Symbol(c)) => Some(c),
            _ => None,
        })
    }
    //

    pub fn lookahead_is_symbol(&mut self, symbol: char) -> TokenizerResult<bool> {
        Ok(self.lookahead_if_symbol()? == Some(symbol))
    }

    pub fn lookahead_is_ident(&mut self, ident: &str) -> TokenizerResult<bool> {
        Ok(match self.lookahead()? {
            Some(Token::Ident(i)) => i == ident,
            _ => false,
        })
    }

    pub fn next_ident(&mut self) -> TokenizerResult<String> {
        let loc = self.lookahead_loc();

        self.next_token_check_map(|token| match token {
            &Token::Ident(ref ident) => Ok(ident.clone()),
            _ => Err(TokenizerError::ExpectIdent(loc)),
        })
    }
    //

    // pub fn token_as_ident(token: Token, loc: Loc) -> TokenizerResult<String> {
    //     match token {
    //         Token::Ident(ident) => Ok(ident),
    //         _ => Err(TokenizerError::ExpectIdent(loc)),
    //     }
    // }

    // pub fn next_token(&mut self) -> TokenizerResult<Token> {
    //     self.lookahead()?;
    //     let next_token = self.next_token.take();
    //     let Some(next_token) = next_token else {
    //         return Err(TokenizerError::UnexpectedEof.into());
    //     };
    //     Ok(next_token.token)
    // }

    pub fn next_str_lit(&mut self) -> TokenizerResult<StrLit> {
        let loc = self.lookahead_loc();
        self.next_token_check_map(|token| match token {
            &Token::StrLit(ref str_lit) => Ok(str_lit.clone()),
            _ => Err(TokenizerError::ExpectStrLit(loc)),
        })
    }

    pub fn next_int_lit(&mut self) -> TokenizerResult<u64> {
        let loc = self.lookahead_loc();
        self.next_token_check_map(|token| match token {
            &Token::IntLit(v) => Ok(v),
            _ => Err(TokenizerError::ExpectIntLit(loc)),
        })
    }

    pub fn next_version(&mut self) -> TokenizerResult<String> {
        let loc = self.lookahead_loc();
        self.next_token_check_map(|token| match token {
            &Token::Version(ref ident) => Ok(ident.clone()),
            _ => Err(TokenizerError::ExpectIdent(loc)),
        })
    }
    //

    pub fn next_float_lit(&mut self) -> TokenizerResult<f64> {
        let loc = self.lookahead_loc();
        self.next_token_check_map(|token| match token {
            &Token::FloatLit(v) => Ok(v),
            _ => Err(TokenizerError::ExpectFloatLit(loc)),
        })
    }
}
