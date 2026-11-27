// Run: NYATERM_PLAYWRIGHT_MODULE=/path/to/playwright node browser.cjs /path/to/nyaterm-mcp
// Uses fixture credentials only. Never prints tokens or client payloads.
const assert = require('node:assert/strict');
const net = require('node:net');
const http = require('node:http');
const {spawn} = require('node:child_process');
const {once} = require('node:events');
const {chromium, firefox} = require(process.env.NYATERM_PLAYWRIGHT_MODULE || 'playwright');
async function listen(server) { server.listen(0, '127.0.0.1'); await once(server, 'listening'); return server.address().port; }
(async () => {
  const sockets = new Set(); let owners = 0; let child;
  const bridge = net.createServer(socket => {
    sockets.add(socket); socket.on('close',()=>sockets.delete(socket));
    const owner = ++owners; let pending = '';
    socket.on('data', chunk => {
      pending += chunk.toString();
      for (;;) {
        const index = pending.indexOf('\n'); if(index < 0) break;
        const request = JSON.parse(pending.slice(0,index)); pending = pending.slice(index+1);
        let result;
        switch(request.method) {
          case 'auth': result = {authenticated:true, features:['client_resume','terminal_lifecycle','connection_targets']}; break;
          case 'client.attach': result = {connectionId:`fixture-${owner}`,resumeToken:owner.toString(16).padStart(64,'0')}; break;
          case 'client.identify': result = {identified:true}; break;
          case 'capability.execute': result = {connections:[]}; break;
          default: result = {};
        }
        socket.write(JSON.stringify({id:request.id,result,error:null})+'\n');
      }
    });
  });
  const pageServer = http.createServer((_, response)=>response.end('<!doctype html><title>MCP HTTP fixture</title>'));
  try {
    const bridgePort = await listen(bridge); const pagePort = await listen(pageServer);
    const reserve = net.createServer(); const port = await listen(reserve); await new Promise(resolve=>reserve.close(resolve));
    child = spawn(process.argv[2],['--transport','streamable-http','--managed'],{stdio:['pipe','pipe','pipe']});
    const exited = once(child,'exit');
    const ready = new Promise((resolve,reject)=>{
      let text=''; const timer=setTimeout(()=>reject(new Error('Helper startup timed out')),12000);
      child.stdout.on('data',chunk=>{text+=chunk; const end=text.indexOf('\n');if(end>=0){clearTimeout(timer);const event=JSON.parse(text.slice(0,end));event.type==='ready'?resolve(event):reject(new Error('Helper failed'));}});
      child.on('error',reject);child.on('exit',()=>{clearTimeout(timer);reject(new Error('Helper exited before ready'));});
    });
    child.stdin.write(JSON.stringify({version:1,port,allowed_origins:[`http://127.0.0.1:${pagePort}`],bridge_port:bridgePort,bridge_token:'fixture',bridge_generation:'fixture'})+'\n');
    const credentials = await ready;
    for (const [name,engine] of [['Chromium',chromium],['Firefox',firefox]]) {
      const browser = await engine.launch({headless:true});
      try {
        const page = await browser.newPage();await page.goto(`http://127.0.0.1:${pagePort}`);
        await page.evaluate(async ({port,token})=>{
          const url=`http://127.0.0.1:${port}/mcp`;
          const headers={'Authorization':`Bearer ${token}`,'Accept':'application/json, text/event-stream','Content-Type':'application/json'};
          const check=(condition,label)=>{if(!condition)throw new Error(label);};
          const message=async payload=>{
            const response=await fetch(url,{method:'POST',headers,body:JSON.stringify(payload)});
            check(response.ok,'MCP request failed'); const text=await response.text();
            const messages=response.headers.get('content-type').startsWith('text/event-stream')?text.split('\n').filter(line=>line.startsWith('data:')&&line.slice(5).trim()).map(line=>JSON.parse(line.slice(5))):[JSON.parse(text)];
            return {response,messages};
          };
          const init=await message({jsonrpc:'2.0',id:1,method:'initialize',params:{protocolVersion:'2025-03-26',capabilities:{},clientInfo:{name:'browser-fixture',version:'1'}}});
          const session=init.response.headers.get('mcp-session-id');check(Boolean(session),'Session header not exposed');headers['Mcp-Session-Id']=session;
          const initialized=await fetch(url,{method:'POST',headers,body:JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})});check(initialized.status===202,'Initialized notification rejected');
          const list=await message({jsonrpc:'2.0',id:2,method:'tools/list'});check(list.messages.some(m=>m.result?.tools?.some(t=>t.name==='connection_list')),'Tool listing failed');
          const call=await message({jsonrpc:'2.0',id:3,method:'tools/call',params:{name:'connection_list',arguments:{}}});check(call.messages.some(m=>Array.isArray(m.result?.structuredContent?.connections)),'Tool call failed');
          const batch=await message([{jsonrpc:'2.0',id:4,method:'ping'},{jsonrpc:'2.0',id:5,method:'ping'}]);check(batch.messages.filter(m=>m.id===4||m.id===5).length===2,'Batch stream failed');
          const removed=await fetch(url,{method:'DELETE',headers});check(removed.ok,'Session deletion failed');
          const expired=await fetch(url,{method:'GET',headers});check(expired.status===404,'Deleted session accepted');
        },credentials);
        console.log(`${name}: browser preflight, initialize, tools/list, tools/call, SSE batch and DELETE passed`);
      } finally {await browser.close();}
    }
    assert.equal(owners,2,'Browser sessions must use separate bridge connections');
    child.stdin.end();await Promise.race([exited,new Promise((_,reject)=>setTimeout(()=>reject(new Error('Helper did not stop on parent EOF')),5000))]);
    console.log('Parent EOF shutdown passed');
  } finally {
    if(child && child.exitCode===null) child.kill('SIGKILL');
    for(const socket of sockets) socket.destroy();bridge.close();pageServer.close();
  }
})().catch(error=>{console.error(error.message);process.exitCode=1;});
