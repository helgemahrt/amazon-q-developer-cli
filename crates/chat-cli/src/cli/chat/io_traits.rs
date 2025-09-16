use std::io::Write;

/// Trait for handling output operations in chat sessions
pub trait ChatOutput {
    type OutWriter: Write + Send;
    type ErrWriter: Write + Send;

    /// Get the stdout writer for structured output (tool results, conversation data)
    fn stdout(&mut self) -> &mut Self::OutWriter;

    /// Get the stderr writer for UI/display output (prompts, errors, formatting)
    fn stderr(&mut self) -> &mut Self::ErrWriter;
}

/// Standard terminal-based I/O implementation
pub struct StandardIO {
    pub stdout: std::io::Stdout,
    pub stderr: std::io::Stderr,
}

impl ChatOutput for StandardIO {
    type ErrWriter = std::io::Stderr;
    type OutWriter = std::io::Stdout;

    fn stdout(&mut self) -> &mut Self::OutWriter {
        &mut self.stdout
    }

    fn stderr(&mut self) -> &mut Self::ErrWriter {
        &mut self.stderr
    }
}

/// Buffered I/O implementation for non-interactive sessions
pub struct BufferedIO {
    pub buffer: Vec<u8>,
}

impl BufferedIO {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
        }
    }
}

impl Default for BufferedIO {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatOutput for BufferedIO {
    type ErrWriter = Vec<u8>;
    type OutWriter = Vec<u8>;

    fn stdout(&mut self) -> &mut Self::OutWriter {
        &mut self.buffer
    }

    fn stderr(&mut self) -> &mut Self::ErrWriter {
        &mut self.buffer
    }
}

pub enum ChatIO {
    StdIO(StandardIO),
    BufferedIO(BufferedIO),
}

impl ChatIO {
    #[allow(clippy::redundant_allocation)]
    pub fn stdout(&mut self) -> Box<&mut (dyn Write + Send)> {
        match self {
            ChatIO::BufferedIO(buffered_io) => Box::new(buffered_io.stdout()),
            ChatIO::StdIO(std_io) => Box::new(std_io.stdout()),
        }
    }

    #[allow(clippy::redundant_allocation)]
    pub fn stderr(&mut self) -> Box<&mut (dyn Write + Send)> {
        match self {
            ChatIO::BufferedIO(buffered_io) => Box::new(buffered_io.stderr()),
            ChatIO::StdIO(std_io) => Box::new(std_io.stderr()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use crossterm::{
        execute,
        style,
    };

    use super::*;

    #[test]
    fn test_standard_io_stdout_write() {
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let mut standard_io = StandardIO { stderr, stdout };

        // Test that stdout writer is accessible and functional
        let result = standard_io.stdout().write(b"test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_standard_io_stderr_write() {
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let mut standard_io = StandardIO { stderr, stdout };

        // Test that stderr writer is accessible and functional
        let result = standard_io.stderr().write(b"error test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_buffered_io_new() {
        let buffered_io = BufferedIO::new();
        assert!(buffered_io.buffer.is_empty());
    }

    #[test]
    fn test_buffered_io_stdout_write() {
        let mut buffered_io = BufferedIO::new();
        let test_data = b"Hello stdout!";

        buffered_io.stdout().write_all(test_data).unwrap();
        assert_eq!(buffered_io.buffer, test_data);
    }

    #[test]
    fn test_buffered_io_stderr_write() {
        let mut buffered_io = BufferedIO::new();
        let test_data = b"Hello stderr!";

        buffered_io.stderr().write_all(test_data).unwrap();
        assert_eq!(buffered_io.buffer, test_data);
    }

    #[test]
    fn test_buffered_io_multiple_writes() {
        let mut buffered_io = BufferedIO::new();

        buffered_io.stdout().write_all(b"First ").unwrap();
        buffered_io.stdout().write_all(b"Second").unwrap();

        assert_eq!(buffered_io.buffer, b"First Second");
    }

    #[test]
    fn test_buffered_io_crossterm_integration() {
        let mut buffered_io = BufferedIO::new();

        execute!(buffered_io.stdout(), style::Print("Hello World!")).unwrap();
        assert_eq!(buffered_io.buffer, b"Hello World!");
    }

    #[test]
    fn test_buffered_io_separate_buffers() {
        let mut buffered_io = BufferedIO::new();

        buffered_io.stdout().write_all(b"stdout data").unwrap();
        buffered_io.stderr().write_all(b"stderr data").unwrap();

        assert_eq!(buffered_io.buffer, b"stdout datastderr data");
    }

    #[test]
    fn test_chat_io_buffered_stdout() {
        let buffered_io = BufferedIO::new();
        let mut chat_io = ChatIO::BufferedIO(buffered_io);

        let mut stdout_writer = chat_io.stdout();
        stdout_writer.write_all(b"test data").unwrap();

        if let ChatIO::BufferedIO(ref buffered) = chat_io {
            assert_eq!(buffered.buffer, b"test data");
        } else {
            panic!("Expected BufferedIO variant");
        }
    }

    #[test]
    fn test_chat_io_buffered_stderr() {
        let buffered_io = BufferedIO::new();
        let mut chat_io = ChatIO::BufferedIO(buffered_io);

        let mut stderr_writer = chat_io.stderr();
        stderr_writer.write_all(b"error data").unwrap();

        if let ChatIO::BufferedIO(ref buffered) = chat_io {
            assert_eq!(buffered.buffer, b"error data");
        } else {
            panic!("Expected BufferedIO variant");
        }
    }

    #[test]
    fn test_chat_io_stdio_stdout() {
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let standard_io = StandardIO { stderr, stdout };
        let mut chat_io = ChatIO::StdIO(standard_io);

        let mut stdout_writer = chat_io.stdout();
        let result = stdout_writer.write(b"test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_chat_io_stdio_stderr() {
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let standard_io = StandardIO { stderr, stdout };
        let mut chat_io = ChatIO::StdIO(standard_io);

        let mut stderr_writer = chat_io.stderr();
        let result = stderr_writer.write(b"error");
        assert!(result.is_ok());
    }

    #[test]
    fn test_buffered_io_empty_write() {
        let mut buffered_io = BufferedIO::new();

        buffered_io.stdout().write_all(b"").unwrap();
        assert!(buffered_io.buffer.is_empty());
    }

    #[test]
    fn test_buffered_io_large_write() {
        let mut buffered_io = BufferedIO::new();
        let large_data = vec![b'x'; 10000];

        buffered_io.stdout().write_all(&large_data).unwrap();
        assert_eq!(buffered_io.buffer.len(), 10000);
        assert_eq!(buffered_io.buffer, large_data);
    }

    #[test]
    fn test_buffered_io_binary_data() {
        let mut buffered_io = BufferedIO::new();
        let binary_data = vec![0u8, 255u8, 128u8, 42u8];

        buffered_io.stdout().write_all(&binary_data).unwrap();
        assert_eq!(buffered_io.buffer, binary_data);
    }

    #[test]
    fn test_buffered_io_flush() {
        let mut buffered_io = BufferedIO::new();

        buffered_io.stdout().write_all(b"test").unwrap();
        let result = buffered_io.stdout().flush();
        assert!(result.is_ok());
        assert_eq!(buffered_io.buffer, b"test");
    }

    #[test]
    fn test_chat_io_enum_pattern_matching() {
        let buffered_io = BufferedIO::new();
        let chat_io = ChatIO::BufferedIO(buffered_io);

        match chat_io {
            ChatIO::BufferedIO(_) => assert!(true),
            ChatIO::StdIO(_) => panic!("Expected BufferedIO variant"),
        }

        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let standard_io = StandardIO { stderr, stdout };
        let chat_io = ChatIO::StdIO(standard_io);

        match chat_io {
            ChatIO::StdIO(_) => assert!(true),
            ChatIO::BufferedIO(_) => panic!("Expected StdIO variant"),
        }
    }

    #[test]
    fn test_buffered_io_concurrent_access() {
        let mut buffered_io = BufferedIO::new();

        // Simulate concurrent-like access patterns
        let stdout_ref = buffered_io.stdout();
        stdout_ref.write_all(b"first").unwrap();

        let stderr_ref = buffered_io.stderr();
        stderr_ref.write_all(b"error").unwrap();

        let stdout_ref2 = buffered_io.stdout();
        stdout_ref2.write_all(b" second").unwrap();

        assert_eq!(buffered_io.buffer, b"firsterror second");
    }

    #[test]
    fn test_write_trait_bounds() {
        fn test_write_bound<W: Write + Send>(_writer: W) {}

        let buffered_io = BufferedIO::new();
        test_write_bound(buffered_io.buffer);
    }
}
