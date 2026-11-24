$ErrorActionPreference = 'Stop'
function Reply($id, $value) { [Console]::Out.WriteLine((@{jsonrpc='2.0';id=$id;result=$value} | ConvertTo-Json -Depth 20 -Compress)) }
function Update($sid, $text) { [Console]::Out.WriteLine((@{jsonrpc='2.0';method='session/update';params=@{sessionId=$sid;update=@{sessionUpdate='agent_message_chunk';content=@{type='text';text=$text}}}} | ConvertTo-Json -Depth 20 -Compress)) }
$scenario = $env:TEST_SCENARIO
$session = 'fixture-session'
$options = @{modes=@{currentModeId='default';availableModes=@(@{id='default';name='Default'})};configOptions=@(@{id='model';name='Model';category='model';type='select';currentValue='fixture';options=@(@{value='fixture';name='Fixture'})})}
$promptId = $null
while ($null -ne ($line = [Console]::ReadLine())) {
    $request = $line | ConvertFrom-Json
    switch ($request.method) {
        'initialize' {
            if ($scenario -eq 'hang-init') { Start-Sleep -Seconds 30 }
            Reply $request.id @{protocolVersion=1;agentCapabilities=@{loadSession=$true;sessionCapabilities=@{resume=@{}}};authMethods=@()}
        }
        'session/new' { $created=$options.Clone();$created.sessionId=$session;Reply $request.id $created }
        'session/resume' {
            if ($scenario -eq 'restore-error') { [Console]::Out.WriteLine((@{jsonrpc='2.0';id=$request.id;error=@{code=-32603;message='fixture refused'}} | ConvertTo-Json -Depth 10 -Compress)); continue }
            Update $session 'REPLAY'
            Reply $request.id $options
        }
        'session/set_mode' { Reply $request.id @{} }
        'session/set_config_option' { Reply $request.id @{configOptions=$options.configOptions} }
        'session/prompt' {
            $promptId = $request.id
            if ($scenario -eq 'eof') { exit 0 }
            if ($scenario -eq 'oversized') { [Console]::Out.WriteLine(('x' * 8388609)); continue }
            if ($scenario -eq 'hang') { continue }
            if ($scenario -eq 'permission' -or $scenario -eq 'foreign-permission') {
                $sid = if ($scenario -eq 'foreign-permission') { 'other-session' } else { $session }
                [Console]::Out.WriteLine((@{jsonrpc='2.0';id='permission';method='session/request_permission';params=@{sessionId=$sid;toolCall=@{toolCallId='tool';title='mcp__nyaterm__misleading';_meta=@{claudeCode=@{toolName='Bash'}}};options=@(@{optionId='allow';name='Allow';kind='allow_once'},@{optionId='deny';name='Deny';kind='reject_once'})}} | ConvertTo-Json -Depth 20 -Compress))
                continue
            }
            Update 'other-session' 'FOREIGN'
            Update $session 'OK'
            Reply $request.id @{stopReason=$(if ($scenario -eq 'refusal') {'refusal'} else {'end_turn'})}
        }
        'session/cancel' { Reply $promptId @{stopReason='cancelled'} }
        default {
            if ($request.id -eq 'permission') {
                if ($request.result.outcome.optionId -eq 'deny') { Update $session 'DENIED' } else {Update $session 'UNSAFE'}
                Reply $promptId @{stopReason='end_turn'}
            }
        }
    }
}
