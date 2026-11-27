complete -c penpen -s m -l model -x -a "(penpen --list-models)" -d 'Select a LLM model' -r
complete -c penpen -l prompt -d 'Use the system prompt'
complete -c penpen -s r -l role -x -a "(penpen --list-roles)" -d 'Select a role' -r
complete -c penpen -s s -l session -x  -a "(penpen --list-sessions)" -d 'Start or join a session' -r
complete -c penpen -l empty-session -d 'Ensure the session is empty'
complete -c penpen -l save-session -d 'Ensure the new conversation is saved to the session'
complete -c penpen -s a -l agent -x  -a "(penpen --list-agents)" -d 'Start a agent' -r
complete -c penpen -l agent-variable -d 'Set agent variables'
complete -c penpen -l rag -x  -a"(penpen --list-rags)" -d 'Start a RAG' -r
complete -c penpen -l rebuild-rag -d 'Rebuild the RAG to sync document changes'
complete -c penpen -l macro -x  -a"(penpen --list-macros)" -d 'Execute a macro' -r
complete -c penpen -l serve -d 'Serve the LLM API and WebAPP'
complete -c penpen -s e -l execute -d 'Execute commands in natural language'
complete -c penpen -s c -l code -d 'Output code only'
complete -c penpen -s f -l file -d 'Include files, directories, or URLs' -r -F
complete -c penpen -s S -l no-stream -d 'Turn off stream mode'
complete -c penpen -l dry-run -d 'Display the message without sending it'
complete -c penpen -l info -d 'Display information'
complete -c penpen -l sync-models -d 'Sync models updates'
complete -c penpen -l list-models -d 'List all available chat models'
complete -c penpen -l list-roles -d 'List all roles'
complete -c penpen -l list-sessions -d 'List all sessions'
complete -c penpen -l list-agents -d 'List all agents'
complete -c penpen -l list-rags -d 'List all RAGs'
complete -c penpen -l list-macros -d 'List all macros'
complete -c penpen -s h -l help -d 'Print help'
complete -c penpen -s V -l version -d 'Print version'
