use crate::lexer::lexer_impl::Lexer;
use crate::lexer::token::Token;

fn trim_last_empty_line(s: &mut String) {
    if let Some(pos) = s.rfind('\n') {
        let last_line = &s[pos + 1..];
        if last_line.trim().is_empty() {
            s.truncate(pos); // 直接砍掉换行符和这一行
        }
    } else if s.trim().is_empty() {
        s.clear(); // 整个字符串就是空的
    }
}

fn is_brace_block_empty(s: &str) -> bool {
    if let Some(pos2) = s.rfind('{') {
        let last_line = &s[pos2 + 1..];
        return last_line.trim().is_empty();
    }
    false
}
pub fn format(code: &str) -> Result<String, String> {
    let mut lexer = Lexer::new_want_comment(code);
    let mut space: i32 = 0;
    let mut builder = String::with_capacity(code.len() * 2);

    let mut is_parse_subscribe = false;

    let mut is_returns = false;

    let align_space = 2;

    while let Ok(token) = lexer.next_token() {
        //
        let Some(token) = token else {
            break;
        };

        match token.token {
            Token::Ident(value) => {
                if value == "subscribe" {
                    is_parse_subscribe = true;
                }

                if value == "returns" {
                    is_returns = true;
                }
                builder += &format!("{} ", value);
            }
            Token::Symbol(value) => match value {
                '{' => {
                    is_parse_subscribe = false;
                    if builder.ends_with(" ") {
                        builder += "{ \n";
                    } else {
                        builder += " { \n";
                    }
                    space += align_space;
                    builder += &" ".repeat(space as usize);
                }
                '}' => {
                    space -= align_space;
                    if space < 0 {
                        space = 0;
                    }
                    trim_last_empty_line(&mut builder);
                    if is_brace_block_empty(&builder) {
                        builder += "\n";
                    }
                    builder += "\n";
                    builder += &" ".repeat(space as usize);
                    builder += "}\n\n";
                    builder += &" ".repeat(space as usize);
                }
                ';' => {
                    is_returns = false;
                    builder.truncate(builder.trim_end().len());
                    builder += ";\n";
                    builder += &" ".repeat(space as usize);
                }
                '(' | ')' => {
                    if !is_returns {
                        builder.truncate(builder.trim_end().len());
                    }
                    is_returns = false;

                    builder += &format!("{}", value);
                }
                '.' | '_' => {
                    builder.truncate(builder.trim_end().len());
                    builder += &format!("{}", value);
                }
                _ => {
                    if builder.ends_with(" ") {
                        builder += &format!("{} ", value);
                    } else {
                        builder += &format!(" {} ", value);
                    }
                }
            },
            Token::IntLit(value) => {
                builder += &value.to_string();
            }
            Token::FloatLit(value) => {
                builder += &value.to_string();
            }
            Token::StrLit(value) => {
                builder += &format!("{}", value);
            }
            //
            Token::Version(value) => {
                if is_parse_subscribe {
                    builder.truncate(builder.trim_end().len());
                    builder += "-";
                }
                builder += &value;
            }
            Token::Annotation(value) => {
                builder += &format!("//@ {}\n", value);
                builder += &" ".repeat(space as usize);
            }
            Token::BlockComment(value) => {
                if value.ends_with("\n") {
                    builder += &format!("/* {}*/\n", value);
                } else {
                    builder += &format!("/* {} \n*/\n", value);
                }
                builder += &" ".repeat(space as usize);
            }
            Token::Comment(value) => {
                if value.starts_with(" ") {
                    builder += &format!("//{}\n", value);
                } else {
                    builder += &format!("// {}\n", value);
                }

                builder += &" ".repeat(space as usize);
            }
        }
    }
    Ok(builder)
}

#[test]
pub fn test_1() {
    let code = r#"
    topic dd {
    }

    subscribe ydsds {

    }

    message dsdasdasd {

    }

    event  ?  ? {
        AddressChange ;
        ChannelConnected ;
        LocalServiceOff ;
        LocalServiceOn ;
        AddressChange ;
        LookForDXC ;
    }

    message dsdfsdfsdfsdf3a44 {
        int32 asdas  = 0;
        // 你好啊
        // #啊大苏打
        int32 asdas  = 2;
        // 你好啊
        int32 asdas  = 2;
        // 你好啊
    }

    message dfsdf {
        int64 dd  = 0;
    }
service AdminFortressService { 
  rpc ListNode(node_service_0_0_1.PagerRequest) returns (node_service_0_0_1.PageResponse);
}

    "#;
    //     let code = r#"
    //             topic {
    //     //@ 通知有新任务
    //     IndexStatistics(IndexStatisticsData);
    //     IndexStatistics(IndexStatisticsData);
    // }
    //  "#;
    let format_code = format(code).unwrap();

    println!("\n===\n{}", format_code);
}
