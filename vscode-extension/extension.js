const { workspace } = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

function activate(context) {
  const config = workspace.getConfiguration('vyrn');
  const serverPath = config.get('serverPath') || 'vyrn';

  const serverOptions = {
    command: serverPath,
    args: ['--lsp']
  };

  const clientOptions = {
    documentSelector: [{ scheme: 'file', language: 'vyrn' }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher('**/*.vyn')
    }
  };

  client = new LanguageClient(
    'vyrn-lsp',
    'Vyrn Language Server',
    serverOptions,
    clientOptions
  );

  context.subscriptions.push(client);
  client.start();
}

function deactivate() {
  if (!client) return;
  return client.stop();
}

module.exports = { activate, deactivate };
