from tinytodo import *
from typing import List

def present(cmds: List[str]):
    print(f'Running {len(cmds)} commands')
    for cmd in cmds:
        input()
        print(">>>", cmd)
        exec(cmd)

if __name__ == '__main__':
    present([
        """start_server('./entities.huge.db')""",
        """set_user(User('c6c0ca05-bd98-8923-4fdc-d62be4b966f8'))""",
        """get_list('List::"8f1e1aa9-81d1-a76e-a4f5-aee6b06430a2"')""",
        """get_lists()""",
        """stop_server()"""
    ])
