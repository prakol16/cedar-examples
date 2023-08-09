from tinytodo import *

def present(cmds: list[str]):
    print(f'Running {len(cmds)} commands')
    for cmd in cmds:
        input()
        print(">>>", cmd)
        exec(cmd)

if __name__ == '__main__':
    present([
        """start_server('./huge_entities.db')""",
        """set_user(User('b02cc63c-9daf-465b-82d9-bff7e113a6d9'))""",
        """get_list('List::"2fe39ac9-c04c-424e-9f70-7083de89e51a"')""",
        """get_lists()""",
        """stop_server()"""
    ])