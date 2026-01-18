pub mod formatter;
pub mod lexer;
pub mod parser;

#[cfg(test)]
mod test {
    use crate::parser::parser_impl::Parser;

    #[test]
    fn test_parse() {
        let content = r#"
  syntax = "proto3";

  message Hello {
    string name = 1;
  }

  message User {
    int32 id = 1;
    string name = 2;
    int32 age = 3;
  }
  
  message DBExecuteResult {
    uint64 last_insert_id = 1;
    uint64 affected_rows = 2;
    message Inner {
      uint64 last_insert_id = 1;
      uint64 affected_rows = 2;
    }
    Inner value = 3;
  }
  
  message HelloReply {
    repeated User users = 1;
  }
  
  service  Demo1 {
      rpc SayHello(Hello) returns (HelloReply);
  
      rpc Insert(User) returns (DBExecuteResult);
  
      rpc Delete(User) returns (DBExecuteResult);
  
      rpc Update(User) returns (DBExecuteResult);
  }
"#;

        let mut parser = Parser::new(content);
        let result = parser.next_proto();

        println!("result = {:?}", result);

        let file_descriptor = parser.next_proto().unwrap();

        for message in file_descriptor.messages.iter() {
            println!("{:?}", message.t);
        }
    }
}
