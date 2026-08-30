# DSEC Discord Bot

Deakin Software Engineering Club Discord Bot project. To encourage students to learn Rust and how to work in a practical and collaborative project.

## Setup

### Setup Rust

Installing Rustup will also install `cargo`

Linux & MacOS:

```
curl https://sh.rustup.rs -sSf | sh
```

Windows:

Download and run [rustup-init.exe](https://win.rustup.rs/)

### Setup Discord Bot Profile on Discord Developers

Note: To contribute, you need to create your own Discord Bot profile and test it yourself in another server.

1. Open [Discord Developers](https://discord.com/developers) and click on "Get Started"
2. Create a New Application, with any name you like
3. Navigate to `Bot` on the left sidebar
   - Note down the `Token`, the code of your bot will require it.
   - Enable all the Intents `Presence`, `Server Members`, `Message Content`
     - This is required by Discord to ensure popular discord bots do not scrape server message contents without permission.
4. Generate a Discord Bot URL:
   1. Navigate to `OAuth2` on the left sidebar
   2. Scroll down to `OAuth2 URL Generator`
   3. Under Scopes, select `bot`
   4. Scroll down to `Bot Permissions`
   5. Select permissions, or later override it in the invite link.
     - DSEC Bot's Permission integer is `4235288712703990`.
   6. Copy the Generated URL, and invite your bot to your Discord server.

    OR 
    1. `https://discord.com/oauth2/authorize?client_id=DISCORD_BOT_ID&permissions=4235288712703990&integration_type=0&scope=bot`

### Setup Discord Bot on your machine

Rust with Cargo
1. Navigate to directory on your machine
2. `git clone https://github.com/liyunze-coding/DSEC-Discord-Bot`
3. Create `.env` file according to `.env.example`
  - You can ask the committee (or Ryan) for the environment variables on Discord.
4. Run `cargo run`
5. You may need to reload Discord to see changes to slash commands.

Or Use Docker
1. Make sure Docker engine is running.
   - On Windows, open Docker Desktop.
2. Run the commands
```
docker-compose build
docker-compose up
```

## Logging

The bot logs at `info` by default. Do **not** set `RUST_LOG` to `debug` or `trace`
on the VPS or in the `DOT_ENV` secret: at `debug` the Supabase client logs the
generated query URLs (which contain **student IDs**) and the service-account
email. Adjust the level with `RUST_LOG` locally only (e.g. `RUST_LOG=warn`).

## Rules

### General Rules
- Follow DSEC Server Rules
- Follow Deakin Code of Conduct
- Follow Discord Terms of Services

### Programming Rules
- Do not test in Production
- Do not write malicious code (unless you have obtained permission for white hat hacking)
- Do not spam pull requests
- Do not add your own code formatter, affecting the whole files you edit

## To-do

- Membership verification command
- Unit information command

## Information 

### What is Rust?

Rust is memory safe yet performant, making it the ideal programming language for systems programming. 

C and C++ require developers to manage memory allocation, which can lead to memory unsafe programs. 

Python, Java, C# and Go use the garbage collector so that developers don't need to manually manage memory, but can slow down the program significantly due to lack of low level control. 

Rust takes a unique approach, by using an "ownership" and "borrowing" system to prevent memory bugs at compile time.

Hence, Rust is performant and memory safe (when you write it well).

### Why Rust?

This is a good opportunity for students at Deakin to learn Rust.

At Deakin, Software Engineering, Computer Science and IT students mostly touch on high level languages such as Python, C# and low level languages such as C++.

More and more developer tools are being written in Rust, including Rolldown, Rspack, Tauri, SWC and many more (Ryan is a web developer, he's only aware of these tools written in Rust).

Rust has its own unique concepts and challenges such as the ownership model and the borrow checker. Its strict rules help prevent programming errors such as data races and memory leaks. The strict rules also help students learn how to think about writing efficient code coming from high level languages.

## Contributors

[Ryan](https://github.com/liyunze-coding)