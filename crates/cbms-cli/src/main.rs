use std::{
    io::Write,
    panic::{self, PanicHookInfo},
    path::PathBuf,
    process::Command,
};

use cbms::{
    ErrorPayload, Message, MessageId, StatusCode,
    transport::{StreamTransport, Transport},
};
use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt, stdin, stdout};

#[derive(Debug, clap::Parser)]
#[command(version, about = "CBOR-based Message Stream (CBMS)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    Encode {
        input: Option<String>,
        #[arg(short, long)]
        input_file: Option<PathBuf>,
        #[arg(short, long)]
        output_file: Option<PathBuf>,
    },
    Decode {
        input: Option<String>,
        #[arg(short, long)]
        input_file: Option<PathBuf>,
        #[arg(short, long)]
        output_file: Option<PathBuf>,
    },
    Exec {
        command: String,
    },
    Tunnel {
        remote: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    panic::set_hook(Box::new(panic_hook));

    let cli = Cli::parse();

    match cli.command {
        Commands::Encode {
            input,
            input_file,
            output_file,
        } => encode_cmd(input, input_file, output_file).await,
        Commands::Decode {
            input,
            input_file,
            output_file,
        } => decode_cmd(input, input_file, output_file).await,
        Commands::Exec { command } => exec_cmd(command).await,
        Commands::Tunnel {} => todo!(),
    };
}

fn panic_hook(info: &PanicHookInfo) {
    const PANIC_STATUS_CODE: StatusCode = cbms::StatusCode::SoftwareError;

    let msg = if let Some(string) = info.payload().downcast_ref::<&str>() {
        string.to_string()
    } else if let Some(string) = info.payload().downcast_ref::<String>() {
        string.clone()
    } else {
        "panic occured".to_string()
    };

    let error_message = Message::error(
        MessageId::new(),
        PANIC_STATUS_CODE,
        ErrorPayload::new(PANIC_STATUS_CODE.as_u8().to_string(), msg.clone()),
    );

    match error_message.to_cbor() {
        Ok(bytes) => match std::io::stdout().write_all(&bytes) {
            Ok(_) => {}
            Err(err) => eprintln!("{}", err),
        },
        Err(err) => eprintln!("{}", err),
    };
    eprintln!("\n{}", msg);

    std::process::exit(70);
}

async fn encode_cmd(
    input: Option<String>,
    input_file: Option<PathBuf>,
    output_file: Option<PathBuf>,
) {
    let json_data = if let Some(file) = input_file {
        tokio::fs::read_to_string(file)
            .await
            .expect("failed to read input file")
    } else if let Some(string) = input {
        string
    } else {
        let mut buf = String::new();
        stdin()
            .read_to_string(&mut buf)
            .await
            .expect("failed to read stdin");
        buf
    };

    let msg: Message = serde_json::from_str(&json_data).expect("failed to parse JSON into Message");

    let cbor_bytes = msg.to_cbor().expect("failed to serialize Message to CBOR");

    if let Some(file) = output_file {
        tokio::fs::write(file, &cbor_bytes)
            .await
            .expect("failed to write CBOR to file");
    } else {
        let mut out = stdout();
        out.write_all(&cbor_bytes)
            .await
            .expect("failed to write CBOR to stdout");
        out.flush().await.unwrap();
    }
}

async fn decode_cmd(
    input: Option<String>,
    input_file: Option<PathBuf>,
    output_file: Option<PathBuf>,
) {
    let data = if let Some(file) = input_file {
        tokio::fs::read(file)
            .await
            .expect("failed to read input file")
    } else if let Some(string) = input {
        string.into_bytes()
    } else {
        let mut buf = Vec::new();
        stdin()
            .read_to_end(&mut buf)
            .await
            .expect("failed to read stdin");
        buf
    };

    let msg = match Message::from_cbor(&data) {
        Ok(msg) => msg,
        Err(err) => {
            eprintln!("Failed to decode CBOR: {}", err);
            std::process::exit(74);
        }
    };

    let json = serde_json::to_string_pretty(&msg).expect("failed to serialize Message to JSON");

    if let Some(file) = output_file {
        tokio::fs::write(file, json)
            .await
            .expect("failed to write output file");
    } else {
        let mut out = stdout();
        out.write_all(json.as_bytes())
            .await
            .expect("failed to write to stdout");
        out.write_all(b"\n").await.unwrap();
        out.flush().await.unwrap();
    }
}

async fn exec_cmd(command: String) {
    let mut parts = command.split_whitespace();
    let prog = match parts.next() {
        Some(p) => p,
        None => {
            eprintln!("No command given");
            std::process::exit(64)
        }
    };

    let args: Vec<&str> = parts.collect();

    let mut child = Command::new(prog)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("failed to spawn child process");

    let child_stdin = child.stdin.take().expect("failed to get child stdin");
    let child_stdout = child.stdout.take().expect("failed to get child stdout");

    // Channels für die Kommunikation
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Message>();
    let (response_tx, response_rx) = std::sync::mpsc::channel::<Message>();

    // Clone für send_task error handling
    let response_tx_send = response_tx.clone();

    // Task 1: CLI stdin (JSON) -> Parse -> Channel
    let stdin_task = tokio::task::spawn_blocking(move || {
        use std::io::{BufReader, Read};

        let stdin = std::io::stdin();
        let mut stdin_reader = BufReader::new(stdin);
        let mut buffer = Vec::new();
        let mut bracket_depth = 0;
        let mut in_string = false;
        let mut escape_next = false;

        loop {
            let mut byte = [0u8; 1];
            match stdin_reader.read_exact(&mut byte) {
                Ok(_) => {
                    buffer.push(byte[0]);
                    let ch = byte[0] as char;

                    if escape_next {
                        escape_next = false;
                        continue;
                    }

                    if ch == '\\' && in_string {
                        escape_next = true;
                        continue;
                    }

                    if ch == '"' {
                        in_string = !in_string;
                    }

                    if !in_string {
                        if ch == '{' {
                            bracket_depth += 1;
                        } else if ch == '}' {
                            bracket_depth -= 1;
                            if bracket_depth == 0 {
                                let json_str = String::from_utf8_lossy(&buffer);
                                match serde_json::from_str::<Message>(&json_str) {
                                    Ok(msg) => {
                                        if msg_tx.send(msg).is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to parse JSON: {}", e);
                                    }
                                }
                                buffer.clear();
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        // msg_tx wird hier automatisch gedroppt
    });

    // Task 2: Channel -> Child stdin (CBOR via Transport)
    let send_task = tokio::task::spawn_blocking(move || {
        let mut transport = StreamTransport::new(std::io::empty(), child_stdin);

        while let Ok(msg) = msg_rx.recv() {
            if let Err(e) = transport.send(&msg) {
                let error_msg = Message::error(
                    MessageId::new(),
                    StatusCode::Protocol,
                    ErrorPayload::new(
                        "transport_error",
                        format!("Failed to send to child process: {}", e),
                    ),
                );

                let _ = response_tx_send.send(error_msg);
                break;
            }
        }
        // response_tx_send wird hier automatisch gedroppt
    });

    // Task 3: Child stdout (CBOR via Transport) -> Channel
    let recv_task = tokio::task::spawn_blocking(move || {
        let mut transport = StreamTransport::new(child_stdout, std::io::sink());

        loop {
            match transport.recv() {
                Ok(msg) => {
                    if response_tx.send(msg).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    // Prüfe ob es ein normales EOF ist
                    let is_eof = match &err {
                        cbms::Error::CborDe(ciborium::de::Error::Io(io_err)) => {
                            io_err.kind() == std::io::ErrorKind::UnexpectedEof
                        }
                        cbms::Error::Io(io_err) => {
                            io_err.kind() == std::io::ErrorKind::UnexpectedEof
                        }
                        _ => false,
                    };

                    if !is_eof {
                        // Nur bei echten Fehlern eine Error-Message senden
                        let error_msg = Message::error(
                            MessageId::new(),
                            StatusCode::Protocol,
                            ErrorPayload::new(
                                "transport_error",
                                format!("Failed to receive from child process: {}", err),
                            ),
                        );
                        let _ = response_tx.send(error_msg);
                    }
                    break;
                }
            }
        }
        // response_tx wird hier automatisch gedroppt
    });

    // Task 4: Channel -> CLI stdout (JSON)
    let stdout_task = tokio::task::spawn_blocking(move || {
        use std::io::Write;

        let mut stdout = std::io::stdout();

        while let Ok(msg) = response_rx.recv() {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if let Err(e) = writeln!(stdout, "{}", json) {
                        eprintln!("Failed to write to stdout: {}", e);
                        break;
                    }
                    if let Err(e) = stdout.flush() {
                        eprintln!("Failed to flush stdout: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Failed to serialize to JSON: {}", e);
                }
            }
        }
        // Beendet wenn alle response_tx gedroppt wurden
    });

    // Warte auf recv_task - wenn der fertig ist, hat child stdout geschlossen
    let _ = recv_task.await;

    // recv_task hat response_tx gedroppt, send_task wird sein response_tx_send auch droppen
    // wenn msg_rx leer ist. Warte auf send_task.
    let _ = send_task.await;

    // Jetzt sind alle response_tx gedroppt, stdout_task wird beenden
    let _ = stdout_task.await;

    // stdin_task läuft noch im Hintergrund, aber das ist ok - er wartet auf user input
    // Der Child-Prozess ist schon fertig, wir können aufräumen
    let _ = child.kill();

    // Nicht auf stdin_task warten - der könnte ewig auf stdin blocken
}
