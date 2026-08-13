use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::str;

use crate::lexer::int;
use crate::lexer::lexer_impl::LexerError;
use crate::lexer::loc::Loc;
use crate::lexer::num_lit::NumLit;
use crate::lexer::str_lit::StrLitDecodeError;
use crate::lexer::token::Token;
use crate::lexer::tokenizer::Tokenizer;
use crate::lexer::tokenizer::TokenizerError;
use crate::parser::get_file_name_from_path;
use crate::parser::get_import_file_absoule_path;
use crate::parser::get_package_name_from_file_name;
use crate::parser::read_file_as_string;

use regex::Regex;
use std::result::Result::Ok;

use super::model::AnyTypeUrl;
use super::model::EnumValue;
use super::model::Enumeration;
use super::model::Event;
use super::model::EventItem;
use super::model::Extension;
use super::model::Field;
use super::model::FieldOrOneOf;
use super::model::FieldType;
use super::model::FileDescriptor;
use super::model::Message;
use super::model::Method;
use super::model::ProtobufAnntation;
use super::model::ProtobufConstant;
use super::model::ProtobufConstantMessage;
use super::model::ProtobufConstantMessageFieldName;
use super::model::ProtobufOption;
use super::model::ProtobufOptionName;
use super::model::ProtobufOptionNameExt;
use super::model::ProtobufOptionNamePart;
use super::model::Rule;
use super::model::Service;
use super::model::SubscribeTopic;
use super::model::Syntax;
use super::model::Topic;
use super::model::WithLoc;

use super::protobuf_ident::ProtobufIdent;
use super::protobuf_path::ProtobufPath;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ParserError {
    #[error("{0}")]
    TokenizerError(#[source] TokenizerError),
    // TODO
    #[error("incorrect input")]
    IncorrectInput,
    // #[error("not UTF-8")]
    // NotUtf8,
    #[error("expecting a constant")]
    ExpectConstant,
    // #[error("unknown syntax")]
    // UnknownSyntax,
    #[error("integer overflow")]
    IntegerOverflow,
    #[error("label not allowed")]
    LabelNotAllowed,
    #[error("label required")]
    LabelRequired,
    #[error("map field not allowed")]
    MapFieldNotAllowed,
    #[error("string literal decode error: {0}")]
    StrLitDecodeError(#[source] StrLitDecodeError),
    #[error("lexer error: {0}")]
    LexerError(#[source] LexerError),
    // #[error("{} 行 {} 列 {} 字段序号重复 ", .0.line, .0.col, .1)]
    // DuplicateNumber(Loc, String),
    // #[error("{} 行 {} 列 {} 文件: {} 不存在", .0.line, .0.col, .1, .1)]
    // NotFound(Loc, String),
    #[error("{} 行 {} 列 {} 读取文件文件: {} 出错", .0.line, .0.col, .1, .1)]
    ReadError(Loc, String),

    #[error("{} 中 {}  字段，类型 {} 未定义", .0, .1, .2)]
    Undefined(String, String, String),
}

impl From<TokenizerError> for ParserError {
    fn from(e: TokenizerError) -> Self {
        ParserError::TokenizerError(e)
    }
}

impl From<StrLitDecodeError> for ParserError {
    fn from(e: StrLitDecodeError) -> Self {
        ParserError::StrLitDecodeError(e)
    }
}

impl From<LexerError> for ParserError {
    fn from(e: LexerError) -> Self {
        ParserError::LexerError(e)
    }
}

impl From<int::Overflow> for ParserError {
    fn from(_: int::Overflow) -> Self {
        ParserError::IntegerOverflow
    }
}

#[derive(Debug, thiserror::Error)]
#[error("at {line}:{col}: {error}")]
pub struct ParserErrorWithLocation {
    #[source]
    pub error: anyhow::Error,
    /// 1-based
    pub line: u32,
    /// 1-based
    pub col: u32,
}

// trait ToU8 {
//     fn to_u8(&self) -> anyhow::Result<u8>;
// }

trait ToI32 {
    fn to_i32(&self) -> anyhow::Result<i32>;
}

trait ToI64 {
    fn to_i64(&self) -> anyhow::Result<i64>;
}

// trait ToChar {
//     fn to_char(&self) -> anyhow::Result<char>;
// }

impl ToI32 for u64 {
    fn to_i32(&self) -> anyhow::Result<i32> {
        if *self <= i32::max_value() as u64 {
            Ok(*self as i32)
        } else {
            Err(ParserError::IntegerOverflow.into())
        }
    }
}

impl ToI32 for i64 {
    fn to_i32(&self) -> anyhow::Result<i32> {
        if *self <= i32::max_value() as i64 && *self >= i32::min_value() as i64 {
            Ok(*self as i32)
        } else {
            Err(ParserError::IntegerOverflow.into())
        }
    }
}

impl ToI64 for u64 {
    fn to_i64(&self) -> anyhow::Result<i64> {
        if *self <= i64::max_value() as u64 {
            Ok(*self as i64)
        } else {
            Err(ParserError::IntegerOverflow.into())
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ParseScope {
    File(String),    // 当前解析处于 文件
    Message(String), // 当前解析处于 消息
    Enum(String),    // 当前解析处于 枚举
    Service(String), // 当前解析处于 服务
}

#[derive(Clone, Debug)]
pub enum DiagnosticsType {
    Warn(String, Loc, Loc),
    Error(String, Loc, Loc),
}

#[derive(Copy, Clone)]
enum MessageBodyParseMode {
    MessageProto3,
    // Oneof,
    // ExtendProto3,
}

impl MessageBodyParseMode {
    fn label_allowed(&self, label: Rule) -> bool {
        match label {
            Rule::Repeated => true,
            Rule::Optional => true,
            Rule::Required => false,
        }
    }

    fn some_label_required(&self) -> bool {
        false
    }

    fn map_allowed(&self) -> bool {
        true
    }

    fn is_most_non_fields_allowed(&self) -> bool {
        true
    }

    fn is_option_allowed(&self) -> bool {
        true
    }
}

#[derive(Default)]
pub(crate) struct MessageBody {
    pub fields: Vec<WithLoc<FieldOrOneOf>>,
    pub reserved_nums: Vec<RangeInclusive<i32>>,
    pub reserved_names: Vec<String>,
    pub messages: Vec<WithLoc<Message>>,
    pub enums: Vec<WithLoc<Enumeration>>,
    pub options: Vec<ProtobufOption>,
    pub extension_ranges: Vec<RangeInclusive<i32>>,
    pub extensions: Vec<WithLoc<Extension>>,
}

trait NumLitEx {
    fn to_option_value(&self, sign_is_plus: bool) -> anyhow::Result<ProtobufConstant>;
}

impl NumLitEx for NumLit {
    fn to_option_value(&self, sign_is_plus: bool) -> anyhow::Result<ProtobufConstant> {
        Ok(match (*self, sign_is_plus) {
            (NumLit::U64(u), true) => ProtobufConstant::U64(u),
            (NumLit::F64(f), true) => ProtobufConstant::F64(f),
            (NumLit::U64(u), false) => {
                ProtobufConstant::I64(int::neg(u).map_err(|_| ParserError::IntegerOverflow)?)
            }
            (NumLit::F64(f), false) => ProtobufConstant::F64(-f),
        })
    }
}

#[derive(Clone)]
pub struct Parser<'a> {
    // 导入文件
    input: &'a str,
    pub import_filedescriptor_map: HashMap<String, FileDescriptor>,
    pub tokenizer: Tokenizer<'a>,
    syntax: Syntax,
    scope_stack: Vec<ParseScope>,
    message_and_enum_map: HashMap<String, ParseScope>,
    annotation: Option<ProtobufAnntation>,
    //
    diagnostics: Vec<DiagnosticsType>,
}

impl<'a> Parser<'a> {
    //
    pub fn new(input: &'a str) -> Parser<'a> {
        Parser {
            import_filedescriptor_map: HashMap::new(),
            tokenizer: Tokenizer::new(input),
            input,
            syntax: Syntax::Proto3,
            scope_stack: Vec::default(),
            message_and_enum_map: HashMap::new(),
            annotation: None,
            diagnostics: Vec::new(),
        }
    }

    pub fn store_error_diagnostics(&mut self, msg: String, start_loc: Loc, end_loc: Loc) {
        self.diagnostics
            .push(DiagnosticsType::Error(msg, start_loc, end_loc));
    }

    pub fn take_diagnostics(&mut self) -> Vec<DiagnosticsType> {
        std::mem::take(&mut self.diagnostics)
    }

    fn gen_full_ident(&self, ident_name: String) -> String {
        let mut full_ident = String::new();
        for scope in self.scope_stack.iter() {
            match scope {
                ParseScope::Message(name) | ParseScope::Enum(name) => {
                    full_ident.push('.');
                    full_ident.push_str(name)
                }
                _ => {}
            }
        }

        if !ident_name.is_empty() {
            if !ident_name.starts_with(".") {
                full_ident.push('.');
            }

            full_ident.push_str(&ident_name);
        }

        full_ident
    }

    fn next_full_ident(&mut self) -> anyhow::Result<ProtobufPath> {
        // let mut full_ident = self.gen_full_ident(String::default());
        let mut full_ident = String::default();
        // https://github.com/google/protobuf/issues/4563
        // if self.tokenizer.next_symbol_if_eq('.')? {
        full_ident.push('.');
        // }

        full_ident.push_str(&self.tokenizer.next_ident()?);
        while self.tokenizer.next_symbol_if_eq('.')? {
            full_ident.push('.');
            full_ident.push_str(&self.tokenizer.next_ident()?);
        }
        Ok(ProtobufPath::new(full_ident))
    }

    // // fullIdent = ident { "." ident }
    // fn next_full_ident_rel(&mut self) -> anyhow::Result<ProtobufRelPath> {
    //     let mut full_ident = String::new();
    //     full_ident.push_str(&self.tokenizer.next_ident()?);
    //     while self.tokenizer.next_symbol_if_eq('.')? {
    //         full_ident.push('.');
    //         full_ident.push_str(&self.tokenizer.next_ident()?);
    //     }
    //     Ok(ProtobufRelPath::new(full_ident))
    // }

    // emptyStatement = ";"
    fn next_empty_statement_opt(&mut self) -> anyhow::Result<Option<()>> {
        if self.tokenizer.next_symbol_if_eq(';')? {
            Ok(Some(()))
        } else {
            Ok(None)
        }
    }

    // messageName = ident
    // enumName = ident
    // messageType = [ "." ] { ident "." } messageName
    // enumType = [ "." ] { ident "." } enumName
    fn next_message_or_enum_type(&mut self) -> anyhow::Result<ProtobufPath> {
        self.next_full_ident()
    }

    // // groupName = capitalLetter { letter | decimalDigit | "_" }
    // fn next_group_name(&mut self) -> anyhow::Result<String> {
    //     // lexer cannot distinguish between group name and other ident
    //     let mut clone = self.clone();
    //     let ident = clone.tokenizer.next_ident()?;
    //     if !ident.chars().next().unwrap().is_ascii_uppercase() {
    //         return Err(ParserError::GroupNameShouldStartWithUpperCase.into());
    //     }
    //     *self = clone;
    //     Ok(ident)
    // }

    // Boolean

    // boolLit = "true" | "false"
    fn next_bool_lit_opt(&mut self) -> anyhow::Result<Option<bool>> {
        Ok(if self.tokenizer.next_ident_if_eq("true")? {
            Some(true)
        } else if self.tokenizer.next_ident_if_eq("false")? {
            Some(false)
        } else {
            None
        })
    }

    // Constant

    fn next_num_lit(&mut self) -> anyhow::Result<NumLit> {
        self.tokenizer
            .next_token_check_map(|token| Ok(token.to_num_lit()?))
    }

    fn next_message_constant_field_name(
        &mut self,
    ) -> anyhow::Result<ProtobufConstantMessageFieldName> {
        if self.tokenizer.next_symbol_if_eq('[')? {
            let n = self.next_full_ident()?;
            if self.tokenizer.next_symbol_if_eq('/')? {
                let prefix = format!("{}", n);
                let full_type_name = self.next_full_ident()?;
                self.tokenizer
                    .next_symbol_expect_eq(']', "message constant")?;
                Ok(ProtobufConstantMessageFieldName::AnyTypeUrl(AnyTypeUrl {
                    prefix,
                    full_type_name,
                }))
            } else {
                self.tokenizer
                    .next_symbol_expect_eq(']', "message constant")?;
                Ok(ProtobufConstantMessageFieldName::Extension(n))
            }
        } else {
            let n = self.tokenizer.next_ident()?;
            Ok(ProtobufConstantMessageFieldName::Regular(n))
        }
    }

    fn next_message_constant(&mut self) -> anyhow::Result<ProtobufConstantMessage> {
        let mut r = ProtobufConstantMessage::default();
        self.tokenizer
            .next_symbol_expect_eq('{', "message constant")?;
        while !self.tokenizer.lookahead_is_symbol('}')? {
            let n = self.next_message_constant_field_name()?;
            let v = self.next_field_value()?;
            r.fields.insert(n, v);
        }
        self.tokenizer
            .next_symbol_expect_eq('}', "message constant")?;
        Ok(r)
    }

    // constant = fullIdent | ( [ "-" | "+" ] intLit ) | ( [ "-" | "+" ] floatLit ) |
    //            strLit | boolLit
    fn next_constant(&mut self) -> anyhow::Result<ProtobufConstant> {
        // https://github.com/google/protobuf/blob/a21f225824e994ebd35e8447382ea4e0cd165b3c/src/google/protobuf/unittest_custom_options.proto#L350
        if self.tokenizer.lookahead_is_symbol('{')? {
            return Ok(ProtobufConstant::Message(self.next_message_constant()?));
        }

        if let Some(b) = self.next_bool_lit_opt()? {
            return Ok(ProtobufConstant::Bool(b));
        }

        if let &Token::Symbol(c) = self.tokenizer.lookahead_some()? {
            if c == '+' || c == '-' {
                self.tokenizer.advance()?;
                let sign = c == '+';
                return Ok(self.next_num_lit()?.to_option_value(sign)?);
            }
        }

        if let Some(r) = self.tokenizer.next_token_if_map(|token| match token {
            &Token::StrLit(ref s) => Some(ProtobufConstant::String(s.clone())),
            _ => None,
        })? {
            return Ok(r);
        }

        match self.tokenizer.lookahead_some()? {
            &Token::IntLit(..) | &Token::FloatLit(..) => {
                return self.next_num_lit()?.to_option_value(true);
            }
            &Token::Ident(..) => {
                return Ok(ProtobufConstant::Ident(self.next_full_ident()?));
            }
            _ => {}
        }

        Err(ParserError::ExpectConstant.into())
    }

    fn next_field_value(&mut self) -> anyhow::Result<ProtobufConstant> {
        if self.tokenizer.next_symbol_if_eq(':')? {
            // Colon is optional when reading message constant.
            self.next_constant()
        } else {
            Ok(ProtobufConstant::Message(self.next_message_constant()?))
        }
    }

    fn next_int_lit(&mut self) -> anyhow::Result<u64> {
        self.tokenizer.next_token_check_map(|token| match token {
            &Token::IntLit(i) => Ok(i),
            _ => Err(ParserError::IncorrectInput.into()),
        })
    }

    // Syntax

    // syntax = "syntax" "=" quote "proto2" quote ";"
    // syntax = "syntax" "=" quote "proto3" quote ";"
    fn next_syntax(&mut self) -> anyhow::Result<Option<Syntax>> {
        if self.tokenizer.next_ident_if_eq("syntax")? {
            self.tokenizer.next_symbol_expect_eq('=', "syntax")?;
            let _syntax_str = self.tokenizer.next_str_lit()?.decode_utf8()?;
            let syntax = Syntax::Proto3;
            self.tokenizer.next_symbol_expect_eq(';', "syntax")?;
            Ok(Some(syntax))
        } else {
            Ok(None)
        }
    }

    // Import Statement

    // import = "import" [ "weak" | "public" ] strLit ";"
    fn next_import_opt(&mut self) -> anyhow::Result<bool> {
        if self.tokenizer.next_ident_if_eq("import")? {
            let start_loc = self.tokenizer.loc();

            let path = self.tokenizer.next_str_lit()?.decode_utf8()?;
            self.tokenizer.next_symbol_expect_eq(';', "import")?;
            // 获取绝对路径才行
            let full_path = get_import_file_absoule_path(&path);
            let Some(full_path) = full_path else {
                let loc = self.tokenizer.loc();
                self.store_error_diagnostics(
                    format!("import 失败: {} 不存在", path),
                    start_loc,
                    loc,
                );

                return Ok(true);

                // return Err(ParserError::NotFound(loc, path).into());
            };
            let file_name = get_file_name_from_path(&full_path).unwrap();
            let package_name = get_package_name_from_file_name(file_name);

            if self.import_filedescriptor_map.contains_key(&package_name) {
                return Ok(true);
            }
            let input = read_file_as_string(&full_path);
            if let Err(_) = input {
                let loc = self.tokenizer.lookahead_loc();
                return Err(ParserError::ReadError(loc, path).into());
            }
            let input = input.unwrap();
            self.parse_import(package_name, input)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn parse_import(&mut self, package_name: String, input: String) -> anyhow::Result<()> {
        let mut import_parser = Parser::new(&input);
        // 放在子解析器中, 防止重复解析
        import_parser.import_filedescriptor_map =
            std::mem::take(&mut self.import_filedescriptor_map);
        //
        let mut file_descriptor = import_parser.next_proto()?;
        // 再一次性取回来
        let import_filedescriptor_map =
            std::mem::take(&mut file_descriptor.import_filedescriptor_map);
        // 插入进去
        for (key, import_filedescriptor) in import_filedescriptor_map.into_iter() {
            //
            self.import_filedescriptor_map
                .insert(key, import_filedescriptor);
        }

        self.import_filedescriptor_map
            .insert(package_name, file_descriptor);

        Ok(())
    }

    // package = "package" fullIdent ";"
    fn next_package_opt(&mut self) -> anyhow::Result<bool> {
        // 跳过 package的内容
        if self.tokenizer.next_ident_if_eq("package")? {
            // 获取到字符串，构建 parser
            // 截取出新的字符串
            let start_loc = self.tokenizer.loc();

            let name = self.tokenizer.next_ident();
            //
            if name.is_err() {
                let _ = self.tokenizer.skip_symbol_until_eq('}');
                self.store_error_diagnostics(
                    "语法错误: package 之后应为包名".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(true);
            };
            let ret = self.tokenizer.next_symbol_expect_eq('{', "package body");
            if ret.is_err() {
                let _ = self.tokenizer.skip_pair_symbol('}');
                //
                self.store_error_diagnostics(
                    "语法错误: package 名之后应为 {".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(true);
            }

            let _ = self.tokenizer.skip_pair_symbol('}');

            Ok(true)
        } else {
            Ok(false)
        }
    }

    // Option

    fn next_ident(&mut self) -> anyhow::Result<ProtobufIdent> {
        Ok(ProtobufIdent::from(self.tokenizer.next_ident()?))
    }

    fn next_option_name_component(&mut self) -> anyhow::Result<ProtobufOptionNamePart> {
        if self.tokenizer.next_symbol_if_eq('(')? {
            let comp = self.next_full_ident()?;
            self.tokenizer
                .next_symbol_expect_eq(')', "option name component")?;
            Ok(ProtobufOptionNamePart::Ext(comp))
        } else {
            Ok(ProtobufOptionNamePart::Direct(self.next_ident()?))
        }
    }

    // https://github.com/google/protobuf/issues/4563
    // optionName = ( ident | "(" fullIdent ")" ) { "." ident }
    fn next_option_name(&mut self) -> anyhow::Result<ProtobufOptionName> {
        let mut components = Vec::new();
        components.push(self.next_option_name_component()?);
        while self.tokenizer.next_symbol_if_eq('.')? {
            components.push(self.next_option_name_component()?);
        }
        if components.len() == 1 {
            if let ProtobufOptionNamePart::Direct(n) = &components[0] {
                return Ok(ProtobufOptionName::Builtin(n.clone()));
            }
        }
        Ok(ProtobufOptionName::Ext(ProtobufOptionNameExt(components)))
    }

    // option = "option" optionName  "=" constant ";"
    fn next_option_opt(&mut self) -> anyhow::Result<Option<ProtobufOption>> {
        if self.tokenizer.next_ident_if_eq("option")? {
            let name = self.next_option_name()?;
            self.tokenizer.next_symbol_expect_eq('=', "option")?;
            let value = self.next_constant()?;
            self.tokenizer.next_symbol_expect_eq(';', "option")?;
            Ok(Some(ProtobufOption { name, value }))
        } else {
            Ok(None)
        }
    }

    fn next_annotation_opt(&mut self) -> anyhow::Result<Option<ProtobufAnntation>> {
        let (is_annotation, annotation) = self.tokenizer.next_token_is_annotation()?;

        if is_annotation {
            // 注解，要放在全局
            Ok(Some(ProtobufAnntation { value: annotation }))
        } else {
            Ok(None)
        }
    }

    // Fields

    // label = "required" | "optional" | "repeated"
    fn next_label(&mut self, mode: MessageBodyParseMode) -> anyhow::Result<Option<Rule>> {
        for rule in Rule::ALL {
            let mut clone = self.clone();
            if clone.tokenizer.next_ident_if_eq(rule.as_str())? {
                if !mode.label_allowed(rule) {
                    return Err(ParserError::LabelNotAllowed.into());
                }

                *self = clone;
                return Ok(Some(rule));
            }
        }

        if mode.some_label_required() {
            Err(ParserError::LabelRequired.into())
        } else {
            Ok(None)
        }
    }

    fn next_field_type(&mut self) -> anyhow::Result<FieldType> {
        //
        let simple = &[
            ("int32", FieldType::Int32),
            ("int64", FieldType::Int64),
            ("uint32", FieldType::Uint32),
            ("uint64", FieldType::Uint64),
            ("sint32", FieldType::Sint32),
            ("sint64", FieldType::Sint64),
            ("fixed32", FieldType::Fixed32),
            ("sfixed32", FieldType::Sfixed32),
            ("fixed64", FieldType::Fixed64),
            ("sfixed64", FieldType::Sfixed64),
            ("bool", FieldType::Bool),
            ("string", FieldType::String),
            ("bytes", FieldType::Bytes),
            ("float", FieldType::Float),
            ("double", FieldType::Double),
        ];
        //
        for &(ref n, ref t) in simple {
            if self.tokenizer.next_ident_if_eq(n)? {
                return Ok(t.clone());
            }
        }
        //
        if let Some(t) = self.next_map_field_type_opt()? {
            return Ok(t);
        }

        let message_or_enum = self.next_message_or_enum_type()?;

        Ok(FieldType::MessageOrEnum(message_or_enum))
    }

    fn next_field_number(&mut self) -> anyhow::Result<i32> {
        // TODO: not all integers are valid field numbers
        self.tokenizer.next_token_check_map(|token| match token {
            &Token::IntLit(i) => i.to_i32(),
            _ => Err(ParserError::IncorrectInput.into()),
        })
    }

    // fieldOption = optionName "=" constant
    fn next_field_option(&mut self) -> anyhow::Result<ProtobufOption> {
        let name = self.next_option_name()?;
        self.tokenizer.next_symbol_expect_eq('=', "field option")?;
        let value = self.next_constant()?;
        Ok(ProtobufOption { name, value })
    }

    // fieldOptions = fieldOption { ","  fieldOption }
    fn next_field_options(&mut self) -> anyhow::Result<Vec<ProtobufOption>> {
        let mut options = Vec::new();

        options.push(self.next_field_option()?);

        while self.tokenizer.next_symbol_if_eq(',')? {
            options.push(self.next_field_option()?);
        }

        Ok(options)
    }

    // field = label type fieldName "=" fieldNumber [ "[" fieldOptions "]" ] ";"
    // group = label "group" groupName "=" fieldNumber messageBody
    fn next_field(&mut self, mode: MessageBodyParseMode) -> anyhow::Result<WithLoc<Field>> {
        let loc = self.tokenizer.lookahead_loc();

        let annotation = self.annotation.take();
        //
        let rule = if self.clone().tokenizer.next_ident_if_eq("map")? {
            if !mode.map_allowed() {
                return Err(ParserError::MapFieldNotAllowed.into());
            }
            None
        } else {
            self.next_label(mode)?
        };

        let typ = self.next_field_type()?;

        let name = self.tokenizer.next_ident()?.to_owned();
        self.tokenizer.next_symbol_expect_eq('=', "field")?;
        let number = self.next_field_number()?;

        let mut options = Vec::new();

        if self.tokenizer.next_symbol_if_eq('[')? {
            for o in self.next_field_options()? {
                options.push(o);
            }
            self.tokenizer.next_symbol_expect_eq(']', "field")?;
        }

        self.tokenizer.next_symbol_expect_eq(';', "field")?;
        let field = Field {
            name,
            rule,
            annotation,
            typ,
            number,
            options,
        };
        Ok(WithLoc { t: field, loc })
    }

    // mapField = "map" "<" keyType "," type ">" mapName "=" fieldNumber [ "[" fieldOptions "]" ] ";"
    // keyType = "int32" | "int64" | "uint32" | "uint64" | "sint32" | "sint64" |
    //           "fixed32" | "fixed64" | "sfixed32" | "sfixed64" | "bool" | "string"
    fn next_map_field_type_opt(&mut self) -> anyhow::Result<Option<FieldType>> {
        if self.tokenizer.next_ident_if_eq("map")? {
            self.tokenizer
                .next_symbol_expect_eq('<', "map field type")?;
            // TODO: restrict key types
            let key = self.next_field_type()?;
            self.tokenizer
                .next_symbol_expect_eq(',', "map field type")?;
            let value = self.next_field_type()?;
            self.tokenizer
                .next_symbol_expect_eq('>', "map field type")?;
            Ok(Some(FieldType::Map(Box::new((key, value)))))
        } else {
            Ok(None)
        }
    }

    // range =  intLit [ "to" ( intLit | "max" ) ]
    fn next_range(&mut self) -> anyhow::Result<RangeInclusive<i32>> {
        let from = self.next_field_number()?;
        let to = if self.tokenizer.next_ident_if_eq("to")? {
            if self.tokenizer.next_ident_if_eq("max")? {
                0x20000000 - 1
            } else {
                self.next_field_number()?
            }
        } else {
            from
        };
        Ok(from..=to)
    }

    // ranges = range { "," range }
    fn next_ranges(&mut self) -> anyhow::Result<Vec<RangeInclusive<i32>>> {
        let mut ranges = Vec::new();
        ranges.push(self.next_range()?);
        while self.tokenizer.next_symbol_if_eq(',')? {
            ranges.push(self.next_range()?);
        }
        Ok(ranges)
    }

    // Grammar is incorrect: https://github.com/google/protobuf/issues/4558
    // reserved = "reserved" ( ranges | fieldNames ) ";"
    // fieldNames = fieldName { "," fieldName }
    fn next_reserved_opt(
        &mut self,
    ) -> anyhow::Result<Option<(Vec<RangeInclusive<i32>>, Vec<String>)>> {
        if self.tokenizer.next_ident_if_eq("reserved")? {
            let (ranges, names) = if let &Token::StrLit(..) = self.tokenizer.lookahead_some()? {
                let mut names = Vec::new();
                names.push(self.tokenizer.next_str_lit()?.decode_utf8()?);
                while self.tokenizer.next_symbol_if_eq(',')? {
                    names.push(self.tokenizer.next_str_lit()?.decode_utf8()?);
                }
                (Vec::new(), names)
            } else {
                (self.next_ranges()?, Vec::new())
            };

            self.tokenizer.next_symbol_expect_eq(';', "reserved")?;

            Ok(Some((ranges, names)))
        } else {
            Ok(None)
        }
    }

    // Top Level definitions

    // Enum definition

    // enumValueOption = optionName "=" constant
    fn next_enum_value_option(&mut self) -> anyhow::Result<ProtobufOption> {
        let name = self.next_option_name()?;
        self.tokenizer
            .next_symbol_expect_eq('=', "enum value option")?;
        let value = self.next_constant()?;
        Ok(ProtobufOption { name, value })
    }

    // https://github.com/google/protobuf/issues/4561
    fn next_enum_value(&mut self) -> anyhow::Result<i32> {
        let minus = self.tokenizer.next_symbol_if_eq('-')?;
        let lit = self.next_int_lit()?;
        Ok(if minus {
            let unsigned = lit.to_i64()?;
            match unsigned.checked_neg() {
                Some(neg) => neg.to_i32()?,
                None => return Err(ParserError::IntegerOverflow.into()),
            }
        } else {
            lit.to_i32()?
        })
    }

    // enumField = ident "=" intLit [ "[" enumValueOption { ","  enumValueOption } "]" ]";"
    fn next_enum_field(&mut self) -> anyhow::Result<EnumValue> {
        let name = self.tokenizer.next_ident()?.to_owned();
        let annotation = self.annotation.take();
        self.tokenizer.next_symbol_expect_eq('=', "enum field")?;
        let number = self.next_enum_value()?;
        let mut options = Vec::new();
        if self.tokenizer.next_symbol_if_eq('[')? {
            options.push(self.next_enum_value_option()?);
            while self.tokenizer.next_symbol_if_eq(',')? {
                options.push(self.next_enum_value_option()?);
            }
            self.tokenizer.next_symbol_expect_eq(']', "enum field")?;
        }

        Ok(EnumValue {
            name,
            number,
            annotation,
            options,
        })
    }

    // enum = "enum" enumName enumBody
    // enumBody = "{" { option | enumField | emptyStatement | reserved } "}"
    fn next_enum_opt(&mut self) -> anyhow::Result<Option<WithLoc<Enumeration>>> {
        let loc = self.tokenizer.lookahead_loc();

        let mut enum_name = String::default();
        self.scope_stack.push(ParseScope::Enum(String::default()));

        let result = if self.tokenizer.next_ident_if_eq("enum")? {
            let annotation = self.annotation.take();

            // 错误恢复：解析 enum 名称
            let name_result = self.tokenizer.next_ident();
            if let Err(err) = name_result {
                let start_loc = err.loc();
                // 跳过到下一个 { 然后跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    "语法错误: enum 名称应为标识符".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                self.scope_stack.pop();
                return Ok(None);
            }
            let name = name_result.unwrap().to_owned();

            enum_name = name.clone();
            self.scope_stack.pop();
            self.scope_stack.push(ParseScope::Enum(name.clone()));

            // 错误恢复：期望 { 开始 enum body
            let brace_result = self.tokenizer.next_symbol_expect_eq('{', "enum");
            if let Err(err) = brace_result {
                let start_loc = err.loc();
                // 尝试找到 { 并跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    format!("语法错误: enum '{}' 之后应为 {{", name),
                    start_loc,
                    self.tokenizer.loc(),
                );
                self.scope_stack.pop();
                return Ok(None);
            }

            let mut values = Vec::new();
            let mut options = Vec::new();
            let mut reserved_nums = Vec::new();
            let mut reserved_names = Vec::new();

            let mut field_num_set = HashSet::new();

            // 解析 enum body（带错误恢复）
            while self.tokenizer.lookahead_if_symbol().unwrap_or(None) != Some('}') {
                // 到文件末尾必须退出。lookahead_if_symbol() 在 EOF 返回 Err，
                // 被 unwrap_or(None) 吞成 None 之后 `None != Some('}')` 恒真，
                // 循环就再也出不来；而下面出错分支每轮都会 push 一条诊断，
                // 表现为 CPU 打满 + 内存无限涨（源文件少一个 '}' 就会触发）。
                if self.tokenizer.syntax_eof().unwrap_or(true) {
                    break;
                }
                match self.next_annotation_opt() {
                    Ok(Some(annotation)) => {
                        self.annotation = Some(annotation);
                        continue;
                    }
                    _ => {}
                }

                // emptyStatement
                if self.tokenizer.next_symbol_if_eq(';').unwrap_or(false) {
                    continue;
                }

                match self.next_reserved_opt() {
                    Ok(Some((field_nums, field_names))) => {
                        reserved_nums.extend(field_nums);
                        reserved_names.extend(field_names);
                        continue;
                    }
                    _ => {}
                }

                match self.next_option_opt() {
                    Ok(Some(o)) => {
                        options.push(o);
                        continue;
                    }
                    _ => {}
                }

                // 尝试解析 enum 字段，失败时跳过
                match self.next_enum_field() {
                    Ok(num_field) => {
                        if !field_num_set.contains(&num_field.number) {
                            field_num_set.insert(num_field.number);
                            values.push(num_field);
                        }
                    }
                    Err(e) => {
                        let start_loc = self.tokenizer.loc();
                        self.skip_to_sync_point_in_message();
                        self.store_error_diagnostics(
                            format!("enum 字段解析错误: {}", e),
                            start_loc,
                            self.tokenizer.loc(),
                        );
                    }
                }
            }

            let _ = self.tokenizer.next_symbol_expect_eq('}', "enum");
            let enumeration = Enumeration {
                name,
                values,
                annotation,
                options,
                reserved_nums,
                reserved_names,
            };

            self.message_and_enum_map.insert(
                self.gen_full_ident(String::default()),
                ParseScope::Enum(String::default()),
            );

            Ok(Some(WithLoc {
                loc,
                t: enumeration,
            }))
        } else {
            Ok(None)
        };

        let scope = self.scope_stack.pop();
        if result.is_ok() && scope.is_some() {
            assert_eq!(scope.unwrap(), ParseScope::Enum(enum_name));
        }

        result
    }

    // Message definition

    // // messageBody = "{" { field | enum | message | extend | extensions | group |
    // //               option | oneof | mapField | reserved | emptyStatement } "}"
    // fn next_message_body(&mut self, mode: MessageBodyParseMode) -> anyhow::Result<MessageBody> {
    //     let mut r = MessageBody::default();
    //     self.tokenizer.next_symbol_expect_eq('{', "message body")?;
    //     // let start_loc = self.tokenizer.loc();
    //     // let ret = self.tokenizer.next_symbol_expect_eq('{', "message body");
    //     // if ret.is_err() {
    //     //     let _ = self.tokenizer.skip_pair_symbol('}');
    //     //     self.store_error_diagnostics(
    //     //         "语法错误: 结构体名之后应为 {".to_string(),
    //     //         start_loc,
    //     //         self.tokenizer.loc(),
    //     //     );
    //     //     return Ok(r);
    //     // }
    //     //

    //     let mut field_num_set = HashSet::new();
    //     while self.tokenizer.lookahead_if_symbol()? != Some('}') {
    //         let loc = self.tokenizer.lookahead_loc();
    //         if let Some(annotation) = self.next_annotation_opt()? {
    //             self.annotation = Some(annotation);
    //             continue;
    //         }
    //         // emptyStatement
    //         if self.tokenizer.next_symbol_if_eq(';')? {
    //             continue;
    //         }
    //         //
    //         if mode.is_most_non_fields_allowed() {
    //             //
    //             if let Some((field_nums, field_names)) = self.next_reserved_opt()? {
    //                 r.reserved_nums.extend(field_nums);
    //                 r.reserved_names.extend(field_names);
    //                 continue;
    //             }
    //             if let Some(nested_message) = self.next_message_opt()? {
    //                 r.messages.push(nested_message);
    //                 continue;
    //             }

    //             if let Some(nested_enum) = self.next_enum_opt()? {
    //                 r.enums.push(nested_enum);
    //                 continue;
    //             }
    //         } else {
    //             self.tokenizer.next_ident_if_eq_error("reserved")?;
    //             self.tokenizer.next_ident_if_eq_error("oneof")?;
    //             self.tokenizer.next_ident_if_eq_error("extend")?;
    //             self.tokenizer.next_ident_if_eq_error("message")?;
    //             self.tokenizer.next_ident_if_eq_error("enum")?;
    //         }

    //         // if mode.is_extensions_allowed() {
    //         //     if let Some(extension_ranges) = self.next_extensions_opt()? {
    //         //         r.extension_ranges.extend(extension_ranges);
    //         //         continue;
    //         //     }
    //         // } else {
    //         self.tokenizer.next_ident_if_eq_error("extensions")?;
    //         // }

    //         if mode.is_option_allowed() {
    //             if let Some(option) = self.next_option_opt()? {
    //                 r.options.push(option);
    //                 continue;
    //             }
    //         } else {
    //             self.tokenizer.next_ident_if_eq_error("option")?;
    //         }

    //         let field = FieldOrOneOf::Field(self.next_field(mode)?);

    //         if let FieldOrOneOf::Field(ref field) = field {
    //             if field_num_set.contains(&field.number) {
    //                 return Err(ParserError::DuplicateNumber(loc, field.name.clone()).into());
    //             }
    //             field_num_set.insert(field.number);
    //         }
    //         r.fields.push(WithLoc { t: field, loc });
    //     }

    //     self.tokenizer.next_symbol_expect_eq('}', "message body")?;

    //     // if ret.is_err() {
    //     //     let _ = self.tokenizer.skip_pair_symbol('}');
    //     //     self.store_error_diagnostics(
    //     //         "语法错误: 结构体名之后应 }".to_string(),
    //     //         start_loc,
    //     //         self.tokenizer.loc(),
    //     //     );
    //     //     return Ok(r);
    //     // }

    //     Ok(r)
    // }

    /// 带错误恢复的 message body 解析
    fn next_message_body_with_recovery(&mut self, mode: MessageBodyParseMode) -> MessageBody {
        let mut r = MessageBody::default();
        // 注意：此时 { 已经被消费了

        let mut field_num_set = HashSet::new();
        while self.tokenizer.lookahead_if_symbol().unwrap_or(None) != Some('}') {
            // EOF 必须退出，理由同 enum body 那处
            if self.tokenizer.syntax_eof().unwrap_or(true) {
                break;
            }
            let loc = self.tokenizer.lookahead_loc();

            // 尝试解析，如果失败则跳过当前项
            if let Ok(Some(annotation)) = self.next_annotation_opt() {
                self.annotation = Some(annotation);
                continue;
            }

            // emptyStatement
            if self.tokenizer.next_symbol_if_eq(';').unwrap_or(false) {
                continue;
            }

            // 尝试解析各种可能的元素，遇到错误时继续
            if mode.is_most_non_fields_allowed() {
                if let Ok(Some((field_nums, field_names))) = self.next_reserved_opt() {
                    r.reserved_nums.extend(field_nums);
                    r.reserved_names.extend(field_names);
                    continue;
                }

                // 嵌套 message（带错误恢复）
                if let Ok(Some(nested_message)) = self.next_message_opt() {
                    r.messages.push(nested_message);
                    continue;
                }

                // 嵌套 enum（带错误恢复）
                if let Ok(Some(nested_enum)) = self.next_enum_opt() {
                    r.enums.push(nested_enum);
                    continue;
                }
            }

            // 跳过不允许的关键字
            let _ = self.tokenizer.next_ident_if_eq_error("extensions");

            if mode.is_option_allowed() {
                if let Ok(Some(option)) = self.next_option_opt() {
                    r.options.push(option);
                    continue;
                }
            }

            // 尝试解析字段，如果失败则跳过到下一个 ; 或 }
            match self.next_field(mode) {
                Ok(field_with_loc) => {
                    if !field_num_set.contains(&field_with_loc.t.number) {
                        field_num_set.insert(field_with_loc.t.number);
                        r.fields.push(WithLoc {
                            t: FieldOrOneOf::Field(field_with_loc),
                            loc,
                        });
                    }
                }
                Err(e) => {
                    // 记录错误并跳过到下一个同步点
                    let start_loc = self.tokenizer.loc();
                    self.skip_to_sync_point_in_message();
                    self.store_error_diagnostics(
                        format!("字段解析错误: {}", e),
                        start_loc,
                        self.tokenizer.loc(),
                    );
                }
            }
        }

        // 消费结束的 }
        let _ = self.tokenizer.next_symbol_expect_eq('}', "message body");

        r
    }

    /// 跳过到下一个同步点（; 或 }）
    fn skip_to_sync_point_in_message(&mut self) {
        while let Ok(true) = self.tokenizer.syntax_eof().map(|eof| !eof) {
            // 检查当前 token
            if let Ok(Some(sym)) = self.tokenizer.lookahead_if_symbol() {
                if sym == ';' {
                    // 消费 ; 并停止
                    let _ = self.tokenizer.advance();
                    break;
                }
                if sym == '}' {
                    // 不消费 }，让调用者处理
                    break;
                }
            }
            // 消费当前 token
            let _ = self.tokenizer.advance();
        }
    }

    // message = "message" messageName messageBody
    fn next_message_opt(&mut self) -> anyhow::Result<Option<WithLoc<Message>>> {
        let loc = self.tokenizer.lookahead_loc();

        let mut message_name = String::default();

        self.scope_stack
            .push(ParseScope::Message(message_name.clone()));

        let result = if self.tokenizer.next_ident_if_eq("message")? {
            let annotation = self.annotation.take();

            // 错误恢复：解析 message 名称
            let name_result = self.tokenizer.next_ident();
            if let Err(err) = name_result {
                let start_loc = err.loc();
                // 跳过到下一个 { 然后跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    format!("语法错误: message 名称应为标识符"),
                    start_loc,
                    self.tokenizer.loc(),
                );
                self.scope_stack.pop();
                return Ok(None);
            }
            let name = name_result.unwrap();

            message_name = name.clone();
            self.scope_stack.pop();
            self.scope_stack.push(ParseScope::Message(name.clone()));

            // 错误恢复：期望 { 开始 message body
            let brace_result = self.tokenizer.next_symbol_expect_eq('{', "message body");
            if let Err(err) = brace_result {
                let start_loc = err.loc();
                // 尝试找到 { 并跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    format!("语法错误: message '{}' 之后应为 {{", name),
                    start_loc,
                    self.tokenizer.loc(),
                );
                self.scope_stack.pop();
                return Ok(None);
            }

            // 解析 message body（带错误恢复）
            let MessageBody {
                fields,
                reserved_nums,
                reserved_names,
                messages,
                enums,
                options,
                extensions,
                extension_ranges,
            } = self.next_message_body_with_recovery(MessageBodyParseMode::MessageProto3);

            let message = Message {
                name,
                fields,
                annotation,
                reserved_nums,
                reserved_names,
                messages,
                enums,
                options,
                extensions,
                extension_ranges,
            };
            self.message_and_enum_map.insert(
                self.gen_full_ident(String::default()),
                ParseScope::Message(String::default()),
            );
            Ok(Some(WithLoc { t: message, loc }))
        } else {
            Ok(None)
        };

        let scope = self.scope_stack.pop();
        if result.is_ok() && scope.is_some() {
            assert_eq!(scope.unwrap(), ParseScope::Message(message_name));
        }

        result
    }

    //
    fn next_event_opt(&mut self) -> anyhow::Result<Option<WithLoc<Event>>> {
        let loc = self.tokenizer.lookahead_loc();

        let result = if self.tokenizer.next_ident_if_eq("event")? {
            let annotation = self.annotation.take();

            // 错误恢复：解析 event 名称
            let name_result = self.tokenizer.next_ident();
            if let Err(err) = name_result {
                let start_loc = err.loc();
                // 跳过到下一个 { 然后跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    "语法错误: event 名称应为标识符".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }
            let name = name_result.unwrap().to_owned();

            // 错误恢复：期望 { 开始 event body
            let brace_result = self.tokenizer.next_symbol_expect_eq('{', "event body");
            if let Err(err) = brace_result {
                let start_loc = err.loc();
                // 尝试找到 { 并跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    format!("语法错误: event '{}' 之后应为 {{", name),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }

            let mut event_items = Vec::new();
            // 解析 event body（带错误恢复）
            while self.tokenizer.lookahead_if_symbol().unwrap_or(None) != Some('}') {
                // EOF 必须退出，理由同 enum body 那处
                if self.tokenizer.syntax_eof().unwrap_or(true) {
                    break;
                }
                if let Ok(Some(annotation)) = self.next_annotation_opt() {
                    self.annotation = Some(annotation);
                    continue;
                }
                if self.tokenizer.next_symbol_if_eq(';').unwrap_or(false) {
                    continue;
                }
                let annotation_item = self.annotation.take();

                // 尝试解析 event name
                match self.tokenizer.next_ident() {
                    Ok(event_name) => {
                        // 期望 ; 结束
                        if self.tokenizer.next_symbol_expect_eq(';', "event").is_ok() {
                            event_items.push(EventItem {
                                name: event_name.to_owned(),
                                annotation: annotation_item,
                            });
                        } else {
                            // 跳过到下一个同步点
                            self.skip_to_sync_point_in_message();
                        }
                    }
                    Err(e) => {
                        let start_loc = e.loc();
                        self.skip_to_sync_point_in_message();
                        self.store_error_diagnostics(
                            format!("event 项解析错误: {}", e),
                            start_loc,
                            self.tokenizer.loc(),
                        );
                    }
                }
            }
            let _ = self.tokenizer.next_symbol_expect_eq('}', "event body");

            let event = Event {
                name,
                annotation,
                event_items,
            };
            Ok(Some(WithLoc { t: event, loc }))
        } else {
            Ok(None)
        };

        result
    }

    // Service definition
    fn next_options_or_colon(&mut self) -> anyhow::Result<Vec<ProtobufOption>> {
        let mut options = Vec::new();
        if self.tokenizer.next_symbol_if_eq('{')? {
            while self.tokenizer.lookahead_if_symbol()? != Some('}') {
                if let Some(option) = self.next_option_opt()? {
                    options.push(option);
                    continue;
                }
                if let Some(()) = self.next_empty_statement_opt()? {
                    continue;
                }
                return Err(ParserError::IncorrectInput.into());
            }
            self.tokenizer.next_symbol_expect_eq('}', "option")?;
        } else {
            self.tokenizer.next_symbol_expect_eq(';', "option")?;
        }

        Ok(options)
    }

    // rpc = "rpc" rpcName "(" [ "stream" ] messageType ")"
    //     "returns" "(" [ "stream" ] messageType ")"
    //     (( "{" { option | emptyStatement } "}" ) | ";" )
    fn next_rpc_opt(&mut self) -> anyhow::Result<Option<Method>> {
        if self.tokenizer.next_ident_if_eq("rpc")? {
            let start_loc = self.tokenizer.loc();
            let name = self.tokenizer.next_ident();
            if name.is_err() {
                self.tokenizer.skip_symbol_until_eq(';');
                self.store_error_diagnostics(
                    "语法错误: rpc 为接口名".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }
            //
            let name = name.unwrap();

            let annotation = self.annotation.take();
            let start_loc = self.tokenizer.loc();
            let ret = self.tokenizer.next_symbol_expect_eq('(', "rpc");

            if ret.is_err() {
                self.tokenizer.skip_symbol_until_eq(';');
                self.store_error_diagnostics(
                    "语法错误: 应为 (".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }

            let input_type = if !self.tokenizer.next_symbol_if_eq(')')? {
                let start_loc = self.tokenizer.loc();
                let input_type = self.next_message_or_enum_type();
                if input_type.is_err() {
                    self.tokenizer.skip_symbol_until_eq(';');
                    self.store_error_diagnostics(
                        "语法错误: 参数类型".to_string(),
                        start_loc,
                        self.tokenizer.loc(),
                    );
                    return Ok(None);
                }
                //
                let input_type = input_type.unwrap();
                let start_loc = self.tokenizer.loc();
                let ret = self.tokenizer.next_symbol_expect_eq(')', "rpc");
                if ret.is_err() {
                    self.tokenizer.skip_symbol_until_eq(';');
                    self.store_error_diagnostics(
                        "语法错误: 应为 )".to_string(),
                        start_loc,
                        self.tokenizer.loc(),
                    );
                    return Ok(None);
                }

                input_type
            } else {
                ProtobufPath::new(String::default())
            };

            let output_type = if self.tokenizer.next_ident_if_eq("returns")? {
                let start_loc = self.tokenizer.loc();

                let ret = self.tokenizer.next_symbol_expect_eq('(', "rpc");
                if ret.is_err() {
                    self.tokenizer.skip_symbol_until_eq(';');
                    self.store_error_diagnostics(
                        "语法错误: 应为 (".to_string(),
                        start_loc,
                        self.tokenizer.loc(),
                    );
                    return Ok(None);
                }

                if !self.tokenizer.next_symbol_if_eq(')')? {
                    let start_loc = self.tokenizer.loc();
                    let output_type = self.next_message_or_enum_type();
                    if output_type.is_err() {
                        self.tokenizer.skip_symbol_until_eq(';');
                        self.store_error_diagnostics(
                            "语法错误: 应为参数类型".to_string(),
                            start_loc,
                            self.tokenizer.loc(),
                        );
                        return Ok(None);
                    }
                    let output_type = output_type.unwrap();
                    let ret = self.tokenizer.next_symbol_expect_eq(')', "rpc");
                    if ret.is_err() {
                        self.tokenizer.skip_symbol_until_eq(';');
                        self.store_error_diagnostics(
                            "语法错误: 应为 )".to_string(),
                            start_loc,
                            self.tokenizer.loc(),
                        );
                        return Ok(None);
                    }
                    output_type
                } else {
                    ProtobufPath::new(String::default())
                }
            } else {
                ProtobufPath::new(String::default())
            };

            let options = self.next_options_or_colon()?;
            Ok(Some(Method {
                name,
                input_type,
                annotation,
                output_type,
                client_streaming: false,
                server_streaming: false,
                options,
            }))
        } else {
            Ok(None)
        }
    }

    fn next_topic_method_opt(&mut self) -> anyhow::Result<Option<Method>> {
        let name = self.tokenizer.next_ident();
        if let Err(err) = name {
            let start_loc = err.loc();
            self.tokenizer.skip_symbol_until_eq(';');
            self.store_error_diagnostics(
                "语法错误: 应为 topic 名，格式错误!".to_string(),
                start_loc,
                self.tokenizer.loc(),
            );
            return Err(err.into());
        }
        let name = name.unwrap();
        let annotation = self.annotation.take();
        let ret = self.tokenizer.next_symbol_expect_eq('(', "topic method");

        if let Err(err) = ret {
            let start_loc = err.loc();
            self.tokenizer.skip_symbol_until_eq(';');
            self.store_error_diagnostics(
                "语法错误: topic 名后应为 (".to_string(),
                start_loc,
                self.tokenizer.loc(),
            );
            return Err(err.into());
        }

        let input_type = if !self.tokenizer.next_symbol_if_eq(')')? {
            let input_type = self.next_message_or_enum_type()?;
            let ret = self.tokenizer.next_symbol_expect_eq(')', "topic method");

            if let Err(err) = ret {
                let start_loc = err.loc();
                self.tokenizer.skip_symbol_until_eq(';');
                self.store_error_diagnostics(
                    "语法错误: topic 的参数应以 ) 结尾".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Err(err.into());
            }
            input_type
        } else {
            ProtobufPath::new(String::default())
        };

        //
        let mut start_pos = self.tokenizer.loc();
        let ret = self.tokenizer.next_symbol_expect_eq(';', "topic method");
        if let Err(err) = ret {
            start_pos.col += 1;
            let mut end_pos = start_pos;
            end_pos.col += 1;
            self.tokenizer.skip_symbol_until_eq(';');
            self.store_error_diagnostics("语法错误: 缺少 ;".to_string(), start_pos, end_pos);
            return Err(err.into());
        }

        Ok(Some(Method {
            name,
            input_type,
            annotation,
            output_type: ProtobufPath::new(String::default()),
            client_streaming: false,
            server_streaming: false,
            options: Vec::default(),
        }))
    }

    // proto2:
    // service = "service" serviceName "{" { option | rpc | stream | emptyStatement } "}"
    //
    // proto3:
    // service = "service" serviceName "{" { option | rpc | emptyStatement } "}"
    fn next_service_opt(&mut self) -> anyhow::Result<Option<WithLoc<Service>>> {
        let loc = self.tokenizer.lookahead_loc();
        if self.tokenizer.next_ident_if_eq("service")? {
            let start_loc = self.tokenizer.loc();
            let name = self.tokenizer.next_ident();
            if name.is_err() {
                self.tokenizer.skip_symbol_until_eq('}');
                self.store_error_diagnostics(
                    "语法错误: service 之后应为服务名".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }

            let name = name.unwrap();
            let annotation = self.annotation.take();
            let mut methods = Vec::new();
            let mut options = Vec::new();
            let start_loc = self.tokenizer.loc();
            let ret = self.tokenizer.next_symbol_expect_eq('{', "service");
            if ret.is_err() {
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    "语法错误: 服务名之后应为 {".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }

            //

            while self.tokenizer.lookahead_if_symbol()? != Some('}') {
                if let Some(annotation) = self.next_annotation_opt()? {
                    self.annotation = Some(annotation);
                    continue;
                }

                if let Some(method) = self.next_rpc_opt()? {
                    methods.push(method);
                    continue;
                }
                if let Some(o) = self.next_option_opt()? {
                    options.push(o);
                    continue;
                }

                if let Some(()) = self.next_empty_statement_opt()? {
                    continue;
                }

                return Err(ParserError::IncorrectInput.into());
            }
            self.tokenizer.next_symbol_expect_eq('}', "service")?;
            Ok(Some(WithLoc {
                loc,
                t: Service {
                    name,
                    methods,
                    annotation,
                    options,
                },
            }))
        } else {
            Ok(None)
        }
    }

    fn next_topic_opt(&mut self) -> anyhow::Result<Option<WithLoc<Topic>>> {
        let loc = self.tokenizer.lookahead_loc();
        if self.tokenizer.next_ident_if_eq("topic")? {
            let mut methods = Vec::new();
            let annotation = self.annotation.take();
            // 处理 { 缺失的错误
            let ret = self.tokenizer.next_symbol_expect_eq('{', "topic");
            if let Err(err) = ret {
                let start_loc = err.loc();
                // 先尝试找到 { 再跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    "语法错误: topic 之后应为 {".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );

                return Ok(None);
            }

            // 解析 topic body（带错误恢复）
            while self.tokenizer.lookahead_if_symbol().unwrap_or(None) != Some('}') {
                // EOF 必须退出，理由同 enum body 那处
                if self.tokenizer.syntax_eof().unwrap_or(true) {
                    break;
                }
                if let Ok(Some(annotation)) = self.next_annotation_opt() {
                    self.annotation = Some(annotation);
                    continue;
                }

                // 尝试解析 method，失败时跳过
                match self.next_topic_method_opt() {
                    Ok(Some(method)) => {
                        methods.push(method);
                        continue;
                    }
                    Ok(None) => {
                        // 无法解析，跳过当前 token
                        let _ = self.tokenizer.advance();
                    }
                    Err(e) => {
                        let start_loc = self.tokenizer.loc();
                        self.skip_to_sync_point_in_message();
                        self.store_error_diagnostics(
                            format!("topic 方法解析错误: {}", e),
                            start_loc,
                            self.tokenizer.loc(),
                        );
                    }
                }
            }
            let _ = self.tokenizer.next_symbol_expect_eq('}', "topic");
            Ok(Some(WithLoc {
                loc,
                t: Topic {
                    methods,
                    annotation,
                },
            }))
        } else {
            Ok(None)
        }
    }

    fn next_subscribe_name(&mut self) -> anyhow::Result<String> {
        if !self.tokenizer.lookahead_is_symbol('{')? {
            //
            let name: String = self.tokenizer.next_ident()?;
            let version = self.tokenizer.next_version()?;
            Ok(format!("{}-{}", name, version))
        } else {
            Ok(String::default())
        }
    }

    fn next_subscribe_topic_opt(&mut self) -> anyhow::Result<Option<WithLoc<SubscribeTopic>>> {
        let loc = self.tokenizer.lookahead_loc();

        if self.tokenizer.next_ident_if_eq("subscribe")? {
            let annotation = self.annotation.take();

            // 错误恢复：解析 subscribe 名称
            let name_result = self.next_subscribe_name();
            if let Err(_err) = name_result {
                let start_loc = self.tokenizer.loc();
                // 跳过到下一个 { 然后跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    "语法错误: subscribe 名称解析失败".to_string(),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }
            let name = name_result.unwrap();

            // 错误恢复：期望 { 开始 subscribe body
            let brace_result = self.tokenizer.next_symbol_expect_eq('{', "subscribe");
            if let Err(err) = brace_result {
                let start_loc = err.loc();
                // 尝试找到 { 并跳过整个块
                self.tokenizer.skip_symbol_until_eq('{');
                let _ = self.tokenizer.skip_pair_symbol('}');
                self.store_error_diagnostics(
                    format!("语法错误: subscribe '{}' 之后应为 {{", name),
                    start_loc,
                    self.tokenizer.loc(),
                );
                return Ok(None);
            }

            let mut methods = Vec::new();

            // 解析 subscribe body（带错误恢复）
            while self.tokenizer.lookahead_if_symbol().unwrap_or(None) != Some('}') {
                // EOF 必须退出，理由同 enum body 那处
                if self.tokenizer.syntax_eof().unwrap_or(true) {
                    break;
                }
                if let Ok(Some(annotation)) = self.next_annotation_opt() {
                    self.annotation = Some(annotation);
                    continue;
                }

                // 尝试解析 method，失败时跳过
                match self.next_topic_method_opt() {
                    Ok(Some(method)) => {
                        methods.push(method);
                        continue;
                    }
                    Ok(None) => {
                        // 无法解析，跳过当前 token
                        let _ = self.tokenizer.advance();
                    }
                    Err(e) => {
                        let start_loc = self.tokenizer.loc();
                        self.skip_to_sync_point_in_message();
                        self.store_error_diagnostics(
                            format!("subscribe 方法解析错误: {}", e),
                            start_loc,
                            self.tokenizer.loc(),
                        );
                    }
                }
            }

            let _ = self.tokenizer.next_symbol_expect_eq('}', "subscribe");
            Ok(Some(WithLoc {
                loc,
                t: SubscribeTopic {
                    name,
                    annotation,
                    methods,
                },
            }))
        } else {
            Ok(None)
        }
    }

    fn get_real_message_type(
        &mut self,
        message_name: &str,
        field_name: &str,
        scope_name: &str,
        field_type: &mut FieldType,
    ) -> anyhow::Result<FieldType> {
        match field_type {
            FieldType::Map(map_type) => {
                match &mut map_type.1 {
                    FieldType::Map(inner_map_type) => {
                        inner_map_type.1 = self.get_real_message_type(
                            message_name,
                            field_name,
                            scope_name,
                            &mut inner_map_type.1,
                        )?;
                    }
                    FieldType::MessageOrEnum(_path) => {
                        map_type.1 = self.get_real_message_type(
                            message_name,
                            field_name,
                            scope_name,
                            &mut map_type.1,
                        )?;
                    }

                    _ => {}
                }
                Ok(FieldType::Map(map_type.clone()))
            }
            FieldType::MessageOrEnum(path) => {
                let path_name = path.get_path();
                let full_name = format!("{}{}", scope_name, path_name);
                //
                let scope = self.message_and_enum_map.get(&full_name);
                let (scope, real_path) = if scope.is_some() {
                    (scope, full_name)
                } else {
                    // 在顶层寻找
                    (self.message_and_enum_map.get(&path_name), path_name.clone())
                };
                if let Some(scope) = scope {
                    match scope {
                        ParseScope::Message(_) => {
                            return Ok(FieldType::Message(ProtobufPath::new(real_path)));
                        }
                        ParseScope::Enum(_) => {
                            return Ok(FieldType::Enum(ProtobufPath::new(real_path)));
                        }
                        _ => Ok(field_type.clone()),
                    }
                } else {
                    //
                    let words: Vec<&str> = path_name.split(".").filter(|e| !e.is_empty()).collect();

                    let import_filedescriptor = self.import_filedescriptor_map.get(words[0]);

                    // println!("keys = {:?}", self.import_filedescriptor_map.keys());
                    if let Some(import_filedescriptor) = import_filedescriptor {
                        if import_filedescriptor.has_enum(words[1]) {
                            return Ok(FieldType::Enum(ProtobufPath::new(real_path)));
                        } else if import_filedescriptor.has_message(words[1]) {
                            return Ok(FieldType::Message(ProtobufPath::new(real_path)));
                        }
                    }

                    return Err(ParserError::Undefined(
                        message_name.to_owned(),
                        field_name.to_owned(),
                        path_name.to_owned(),
                    )
                    .into());
                }
            }
            _ => Ok(field_type.clone()),
        }
    }
    // 更新字段
    fn update_message_field_message_or_enum_type(
        &mut self,
        parent_name: String,
        messages: &mut Vec<WithLoc<Message>>,
    ) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        for message in messages.iter_mut() {
            let message = &mut message.t;

            let scope_name = format!("{}.{}", parent_name, message.name);

            for field in message.fields.iter_mut() {
                let field = &mut field.t;

                match field {
                    FieldOrOneOf::Field(field) => {
                        let field = &mut field.t;

                        field.typ = self.get_real_message_type(
                            &message.name,
                            &field.name,
                            &scope_name,
                            &mut field.typ,
                        )?;
                    }
                    _ => {}
                }
            }

            self.update_message_field_message_or_enum_type(scope_name, &mut message.messages)?;
        }
        Ok(())
    }

    fn get_package_name(content: &str, start_pos: usize) -> (String, usize) {
        let mut end_pos = start_pos;
        let mut is_first_space = true;
        let mut real_start_pos = start_pos;
        for (index, ch) in content.char_indices() {
            if index <= start_pos {
                continue;
            }
            if ch == ' ' {
                if !is_first_space {
                    end_pos += 1;
                    break;
                }
                continue;
            }
            if is_first_space {
                real_start_pos = end_pos;
            }
            end_pos = index;
            is_first_space = false;
        }

        let package_name = content[real_start_pos..end_pos].trim().to_string();
        (package_name, end_pos)
    }

    // 截取 package 内容
    fn extract_package_content(content: &str, start_pos: usize) -> String {
        let mut quote_num = 0;
        let mut end_pos = start_pos;
        let mut real_start_pos = 0;
        //
        for (index, ch) in content.char_indices() {
            if index <= start_pos {
                continue;
            }
            if ch == '{' {
                quote_num += 1;
                if quote_num == 1 {
                    real_start_pos = index + 1;
                }
            } else if ch == '}' {
                quote_num -= 1;
                if quote_num == 0 {
                    end_pos += 1;
                    break;
                }
            }
            end_pos = index;
        }

        if quote_num != 0 {
            return String::default();
        }

        content[real_start_pos..end_pos].to_string()
    }

    fn parse_package(&mut self) -> anyhow::Result<()> {
        // 过滤注释
        let mut content = string_builder::Builder::new(self.input.chars().count());
        for line in self.input.lines() {
            match line.find("//") {
                Some(pos) => {
                    let sub_content = &line[..pos];
                    if !sub_content.is_empty() {
                        content.append(sub_content);
                        content.append("\n");
                    }
                }
                None => {
                    content.append(line);
                    content.append("\n");
                }
            };
        }

        let content = content.string().unwrap();

        let package_reg = Regex::new("package ").unwrap();

        for mat in package_reg.find_iter(&content) {
            let new_end = mat.end();

            let (inner_package_name, name_end_pos) = Self::get_package_name(&content, new_end);

            let content = Self::extract_package_content(&content, name_end_pos);
            if content.is_empty() {
                break;
            }

            self.parse_import(inner_package_name, content)?;
        }
        Ok(())
    }

    // Proto file
    // proto = syntax { import | package | option | topLevelDef | emptyStatement }
    // topLevelDef = message | enum | extend | service
    pub fn next_proto(&mut self) -> anyhow::Result<FileDescriptor> {
        //
        self.scope_stack.push(ParseScope::File(String::from(".")));
        let syntax = self.next_syntax()?.unwrap_or(Syntax::Proto3);
        self.syntax = syntax;
        // let mut imports = Vec::new();
        // let mut package = ProtobufAbsPath::root();
        let mut messages = Vec::new();
        let mut events = Vec::new();
        let mut enums = Vec::new();
        // let mut extensions = Vec::new();
        // let mut options = Vec::new();
        let mut services = Vec::new();
        let mut topics = Vec::new();
        let mut subscribe_topics = Vec::new();

        // 先解析素有的 package 内容先
        self.parse_package()?;

        //
        while !self.tokenizer.syntax_eof()? {
            //

            if self.next_import_opt()? {
                //
                continue;
            }

            if self.next_package_opt()? {
                continue;
            }
            if let Some(annotation) = self.next_annotation_opt()? {
                self.annotation = Some(annotation);
                continue;
            }
            // if let Some(option) = self.next_option_opt()? {
            //     options.push(option);
            //     continue;
            // }

            // 尝试解析 message，遇到错误时继续
            let msg_loc = self.tokenizer.lookahead_loc();
            match self.next_message_opt() {
                Ok(Some(message)) => {
                    messages.push(message);
                    continue;
                }
                Ok(None) => {
                    // 检查是否是因为检测到了 "message" 关键字但解析失败
                    // 如果当前位置向前移动了，说明已经处理了一些内容
                    let new_loc = self.tokenizer.loc();
                    if new_loc.line > msg_loc.line
                        || (new_loc.line == msg_loc.line && new_loc.col > msg_loc.col)
                    {
                        // 已经跳过了一些内容��继续下一次循环
                        continue;
                    }
                    // 否则继续尝试解析其他结构
                }
                Err(e) => {
                    // 记录错误并跳过当前 token
                    let loc = self.tokenizer.loc();
                    self.store_error_diagnostics(
                        format!("解析 message 出错: {}", e),
                        loc,
                        self.tokenizer.loc(),
                    );
                    // 跳过当前 token 继续解析
                    let _ = self.tokenizer.advance();
                    continue;
                }
            }

            if let Some(event) = self.next_event_opt()? {
                events.push(event);
                continue;
            }

            if let Some(enumeration) = self.next_enum_opt()? {
                enums.push(enumeration);
                continue;
            }
            // if let Some(more_extensions) = self.next_extend_opt()? {
            //     extensions.extend(more_extensions);
            //     continue;
            // }
            if let Some(service) = self.next_service_opt()? {
                services.push(service);
                continue;
            }

            if let Some(topic) = self.next_topic_opt()? {
                topics.push(topic);
                continue;
            }

            if let Some(subscribe_topic) = self.next_subscribe_topic_opt()? {
                subscribe_topics.push(subscribe_topic);
                continue;
            }

            if self.tokenizer.next_symbol_if_eq(';')? {
                continue;
            }

            // 跳过无法识别的 token，继续解析
            let _ = self.tokenizer.advance();
        }

        let scope = self.scope_stack.pop().unwrap();

        assert_eq!(scope, ParseScope::File(String::from(".")));
        //
        self.update_message_field_message_or_enum_type(String::default(), &mut messages)?;

        let import_filedescriptor_map = std::mem::take(&mut self.import_filedescriptor_map);

        Ok(FileDescriptor {
            // imports,
            // package,
            syntax,
            messages,
            enums,
            // extensions,
            services,
            // options,
            events,
            topics,
            subscribe_topics,
            import_filedescriptor_map,
        })
    }
}
