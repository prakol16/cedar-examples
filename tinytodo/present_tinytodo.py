from tinytodo import *

def present(cmds: list[str]):
    print(f'Running {len(cmds)} commands')
    for cmd in cmds:
        input()
        print(">>>", cmd)
        exec(cmd)

if __name__ == '__main__':
    present([
        """start_server('./entities.json')""",
        """set_user(User('emina'))""",
        """get_list('List::"l0"')  # Emina has permission to read the list""",
        """toggle_task('List::"l0"', 1)  # Emina also has permission to update the list""",
        """get_list('List::"l0"')""",
        """stop_server()""",
        """start_server('./entities.huge.json')""",
        """set_user(User('b02cc63c-9daf-465b-82d9-bff7e113a6d9'))""",
        """get_list('List::"2fe39ac9-c04c-424e-9f70-7083de89e51a"')""",
        """get_lists()""",
        """stop_server()"""
    ])
