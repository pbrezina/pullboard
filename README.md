# Pullboard

**Your reviews, pull requests, and JIRA work in one place.**

Pullboard collects review requests and your open PRs from multiple forge
accounts and instances in one local dashboard. Track feedback, create review
tickets, and sync PRs to JIRA Cloud by creating or linking tickets and adding
them to sprints. AI can help draft ticket descriptions and estimates; you review
and confirm changes before they are saved.

Supported forges:
* Github

> [!WARNING]
> This was entirely written by AI without human properly reviewing the code.
> Use at your own risk.

## Install (Fedora)

```sh
sudo dnf install git nodejs24-bin nodejs24-npm-bin nano
git clone https://github.com/pbrezina/pullboard.git
cd pullboard
npm ci
```

## Configure

```sh
cp public/assets/config.example.json public/assets/config.json
chmod 600 public/assets/config.json
vi public/assets/config.json
```

## Start

```sh
npm start
```

<http://127.0.0.1:4200>

> [!NOTE]
> Keep the app local: it serves your API credentials. Do not expose it to the network.

### Start without saving changes to JIRA

```sh
npm run start:test
```

## Documentation

- [Configuration and credentials](docs/src/user/configuration.md)
- [Dashboard and workflows](docs/src/user/dashboard.md)
- [Test mode](docs/src/user/test-mode.md)
- [Full documentation](docs/src/introduction.md)
