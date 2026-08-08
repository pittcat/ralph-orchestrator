"""Nowledge Mem Ralph plugin runtime scripts.

This package hosts the hook runtime, recall, memory policy, and writer modules.
Hook entry points dispatch by ``argv[1]`` event name and rely on
``resolve_nowledge_env`` to gate on Ralph loop env (see ``hook_runtime.py``).
"""