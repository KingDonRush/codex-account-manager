// This runs only on our own extension page, never inside a login/website.
chrome.tabs.getCurrent().then(tab => chrome.runtime.sendMessage({type:"bootstrap",tabId:tab.id}));
