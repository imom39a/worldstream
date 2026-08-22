window.__WORLDSTREAM_BROWSER_TRACE__=[];
window.__WORLDSTREAM_HOLD_OBSERVATIONS__=false;
window.__WORLDSTREAM_OBSERVATION_QUEUE__=[];
(function(){
  const trace=window.__WORLDSTREAM_BROWSER_TRACE__;
  const originalFetch=window.fetch.bind(window);
  window.fetch=async function(input,init){
    const headers=new Headers(init&&init.headers);
    trace.push({kind:"fetch",url:String(input),method:(init&&init.method)||"GET",has_authorization:headers.has("authorization")});
    const response=await originalFetch(input,init);
    trace.push({kind:"fetch-response",url:String(input),status:response.status});
    return response;
  };
  const NativeWebSocket=window.WebSocket;
  function TracedWebSocket(url,protocols){
    const socket=new NativeWebSocket(url,protocols);
    socket.addEventListener("close",(event)=>trace.push({kind:"close",code:event.code,was_clean:event.wasClean}));
    socket.addEventListener("error",()=>trace.push({kind:"websocket-error"}));
    let handler=null;
    const proxy=new Proxy(socket,{get(target,property){if(property==="onmessage")return handler;return Reflect.get(target,property,target);},set(target,property,value){if(property==="onmessage"){handler=value;target.onmessage=function(event){let type=null;let safe={};try{const parsed=typeof event.data==="string"?JSON.parse(event.data):null;type=parsed?.type;if(type==="observation.deliver"){const body=parsed?.body||{};const observation=body.observation||{};const activity=observation.activity||observation;const offers=Array.isArray(activity.action_offers)?activity.action_offers:[];safe={frame_seq:typeof body.frame_seq==="number"?body.frame_seq:null,cause_room_seq:typeof body.cause_room_seq==="number"?body.cause_room_seq:null,phase:typeof activity.phase==="string"?activity.phase:null,offer_types:offers.map((offer)=>offer?.action_type).filter((value)=>typeof value==="string")};}else if(type==="error"){safe={error_code:typeof parsed?.body?.code==="string"?parsed.body.code:null,retryable:parsed?.body?.retryable===true};}else if(type==="observation.acked"){safe={cursor:typeof parsed?.body?.cursor==="number"?parsed.body.cursor:null};}}catch{}trace.push({kind:"receive",type,...safe});if(type==="observation.deliver"&&window.__WORLDSTREAM_HOLD_OBSERVATIONS__){window.__WORLDSTREAM_OBSERVATION_QUEUE__.push(event);return;}if(typeof handler==="function")handler.call(proxy,event);};return true;}return Reflect.set(target,property,value);}});
    trace.push({kind:"websocket",url:String(url),protocols:Array.isArray(protocols)?protocols:[protocols]});
    const send=socket.send.bind(socket);
    proxy.send=function(data){let type=null;let safe={};try{const parsed=typeof data==="string"?JSON.parse(data):null;type=parsed?.type;if(type==="observation.ack")safe={through_frame_seq:typeof parsed?.body?.through_frame_seq==="number"?parsed.body.through_frame_seq:null};}catch{}trace.push({kind:"send",ticket_first:typeof data==="string"&&/^wst1:[0-9a-f]{64}$/.test(data),type,...safe});return send(data);};
    return proxy;
  }
  TracedWebSocket.prototype=NativeWebSocket.prototype;
  Object.setPrototypeOf(TracedWebSocket,NativeWebSocket);
  window.WebSocket=TracedWebSocket;
})();
