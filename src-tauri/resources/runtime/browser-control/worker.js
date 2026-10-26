// Browser owns tab identity and closure. No website permissions, content
// scripts, debugger, cookies, HTTP interception or evaluation facility.
let port;
let queue = Promise.resolve();
const bootstrap = new Set();
const operations = new Map();
const isOperation = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
function checkedUrl(value) {
  if (typeof value !== "string" || value.length > 16384) throw Error("invalid_url");
  const url = new URL(value);
  if (!['http:','https:'].includes(url.protocol) || url.username || url.password) throw Error("invalid_url");
  return url.href;
}
async function createTab(url, active) {
  const windows = await chrome.windows.getAll();
  if (windows.length === 0) {
    const window = await chrome.windows.create({url, focused:true});
    if (!Number.isSafeInteger(window.tabs?.[0]?.id)) throw Error("missing_tab");
    return window.tabs[0].id;
  }
  return (await chrome.tabs.create({url, active})).id;
}
async function existingIds() {
  // Without tabs/host permission this returns IDs, not website URLs/titles.
  return new Set((await chrome.tabs.query({})).map(tab=>tab.id));
}
async function closeIds(ids) {
  const existing = await existingIds();
  for (const id of ids) if (existing.has(id)) await chrome.tabs.remove(id);
}
async function dispatch(request) {
  const {method,operation} = request;
  if (['createOperation','operationStatus','closeOperation'].includes(method) && !isOperation(operation)) throw Error('invalid_operation');
  if (method === 'createOperation') {
    if (!Array.isArray(request.urls) || request.urls.length < 1 || request.urls.length > 2) throw Error('invalid_targets');
    const urls = request.urls.map(checkedUrl);
    let entry = operations.get(operation);
    if (entry && JSON.stringify(entry.urls) !== JSON.stringify(urls)) throw Error('operation_conflict');
    if (!entry) {
      if (operations.size >= 16) throw Error('operation_limit');
      entry = {urls,ids:[]};
      // Reservation precedes every effect, even if a later create/ack fails.
      operations.set(operation,entry);
    }
    for (let index = entry.ids.length; index < urls.length; index++) entry.ids.push(await createTab(urls[index],index===0));
    await closeIds([...bootstrap]);
    bootstrap.clear();
    return {};
  }
  if (method === 'operationStatus') {
    const existing = await existingIds();
    return {alive:(operations.get(operation)?.ids ?? []).some(id=>existing.has(id))};
  }
  if (method === 'closeOperation') {
    await closeIds(operations.get(operation)?.ids ?? []);
    operations.delete(operation);
    return {empty:(await existingIds()).size===0};
  }
  if (method === 'openManual') {
    await createTab(checkedUrl(request.url),true);
    return {};
  }
  if (method === 'closeBrowser') {
    // Only the manager's explicit whole-app exit sends this command.
    await closeIds([...await existingIds()]);
    operations.clear();
    return {};
  }
  throw Error('unknown_command');
}
chrome.runtime.onMessage.addListener((message,sender) => {
  if (sender.id !== chrome.runtime.id || sender.url !== chrome.runtime.getURL('bootstrap.html') || message.type !== 'bootstrap' || !Number.isSafeInteger(message.tabId)) return;
  if (sender.tab && message.tabId !== sender.tab.id) return;
  bootstrap.add(message.tabId);
  if (port) return;
  port = chrome.runtime.connectNative('com.codex.account_manager.browser');
  port.onMessage.addListener(request => {
    if (!Number.isSafeInteger(request.id) || request.id <= 0) return;
    queue = queue.then(async () => {
      try {port.postMessage({id:request.id,result:await dispatch(request)});}
      catch (_) {try {port.postMessage({id:request.id,error:'native_action_failed'});} catch (_) {}}
    });
  });
  port.onDisconnect.addListener(()=>{port=undefined;});
  port.postMessage({event:'ready',protocol:1});
});
